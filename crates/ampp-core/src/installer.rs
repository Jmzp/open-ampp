//! Downloading, verifying and extracting catalog packages.

use std::fs;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::catalog::{ArchiveKind, Package};
use crate::error::{Error, IoContext, Result};
use crate::paths::{AppPaths, Tool};

/// How many downloaded bytes between progress notifications.
const PROGRESS_STEP: u64 = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Downloading,
    Verifying,
    Extracting,
    Done,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub tool: Tool,
    pub version: String,
    pub phase: Phase,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Installs `pkg` into `bin/<tool>/<version>` and returns that path.
/// An existing installation of the same version is replaced.
pub async fn install(paths: &AppPaths, pkg: &Package, on_progress: impl Fn(Progress)) -> Result<PathBuf> {
    let report = |phase, downloaded, total| {
        on_progress(Progress { tool: pkg.tool, version: pkg.version.clone(), phase, downloaded, total })
    };

    let downloads = paths.downloads();
    fs::create_dir_all(&downloads).at(&downloads)?;
    let archive = downloads.join(format!("{}-{}.part", pkg.tool.as_str(), pkg.version));

    let result = async {
        let digest = download(&pkg.url, pkg.size, &archive, |done, total| report(Phase::Downloading, done, total)).await?;

        report(Phase::Verifying, 0, None);
        if !digest.eq_ignore_ascii_case(&pkg.sha256) {
            return Err(Error::Checksum {
                file: pkg.url.clone(),
                expected: pkg.sha256.clone(),
                actual: digest,
            });
        }

        report(Phase::Extracting, 0, None);
        let dest = paths.bin(pkg.tool, &pkg.version);
        let (kind, archive, dest_c) = (pkg.archive, archive.clone(), dest.clone());
        tokio::task::spawn_blocking(move || extract_into(kind, &archive, &dest_c))
            .await
            .expect("the extraction task must not panic")?;
        Ok(dest)
    }
    .await;

    let _ = fs::remove_file(&archive);
    if result.is_ok() {
        report(Phase::Done, 0, None);
    }
    result
}

/// Deletes the folder of an installed version.
pub fn uninstall(paths: &AppPaths, tool: Tool, version: &str) -> Result<()> {
    let dir = paths.bin(tool, version);
    if !dir.is_dir() {
        return Err(Error::NotInstalled { tool, version: version.to_string() });
    }
    fs::remove_dir_all(&dir).at(&dir)
}

/// Downloads a single file to `dest`, verifying its SHA-256. `dest` is only replaced if valid.
pub async fn fetch_file(url: &str, sha256: &str, dest: &Path) -> Result<()> {
    let dir = dest.parent().expect("dest tiene padre");
    fs::create_dir_all(dir).at(dir)?;
    let part = dest.with_extension("part");
    let result = async {
        let digest = download(url, None, &part, |_, _| {}).await?;
        if !digest.eq_ignore_ascii_case(sha256) {
            return Err(Error::Checksum { file: url.to_string(), expected: sha256.to_string(), actual: digest });
        }
        fs::rename(&part, dest).at(dest)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Downloads to `dest` hashing on the fly. Returns the SHA-256 in hex.
async fn download(url: &str, size: Option<u64>, dest: &Path, on_chunk: impl Fn(u64, Option<u64>)) -> Result<String> {
    let mut file = tokio::fs::File::create(dest).await.at(dest)?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut last_report = 0u64;
    let mut write = async |chunk: &[u8], total: Option<u64>| -> Result<()> {
        hasher.update(chunk);
        file.write_all(chunk).await.at(dest)?;
        done += chunk.len() as u64;
        if done - last_report >= PROGRESS_STEP || Some(done) == total {
            last_report = done;
            on_chunk(done, total);
        }
        Ok(())
    };

    if let Some(local) = url.strip_prefix("file://") {
        let mut src = tokio::fs::File::open(local).await.at(local)?;
        let total = src.metadata().await.at(local)?.len();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = src.read(&mut buf).await.at(local)?;
            if n == 0 {
                break;
            }
            write(&buf[..n], Some(total)).await?;
        }
    } else {
        let http = |source| Error::Http { url: url.to_string(), source };
        let resp = reqwest::get(url).await.and_then(|r| r.error_for_status()).map_err(http)?;
        let total = resp.content_length().or(size);
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            write(&chunk.map_err(http)?, total).await?;
        }
    }

    file.flush().await.at(dest)?;
    Ok(hex::encode(hasher.finalize()))
}

/// Extracts into a staging directory next to `dest` and moves it into place.
/// A single root folder in the archive (e.g. `httpd-2.4.62/`) is flattened.
pub(crate) fn extract_into(kind: ArchiveKind, archive: &Path, dest: &Path) -> Result<()> {
    let parent = dest.parent().expect("dest tiene padre");
    fs::create_dir_all(parent).at(parent)?;
    let name = dest.file_name().expect("dest tiene nombre").to_string_lossy();
    let staging = parent.join(format!(".{name}.staging"));
    if staging.exists() {
        fs::remove_dir_all(&staging).at(&staging)?;
    }
    fs::create_dir_all(&staging).at(&staging)?;

    let result = (|| {
        match kind {
            ArchiveKind::Zip => extract_zip(archive, &staging)?,
            ArchiveKind::TarGz => extract_tar_gz(archive, &staging)?,
        }
        let root = single_root_dir(&staging)?.unwrap_or_else(|| staging.clone());
        if dest.exists() {
            fs::remove_dir_all(dest).at(dest)?;
        }
        fs::rename(&root, dest).at(dest)
    })();

    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn extract_zip(archive: &Path, out: &Path) -> Result<()> {
    let bad = |reason: String| Error::Archive { path: archive.to_path_buf(), reason };
    let file = fs::File::open(archive).at(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
        // `enclosed_name` rejects absolute paths and `..` (zip slip).
        let rel = entry.enclosed_name().ok_or_else(|| {
            let name = entry.name();
            bad(tr!("ruta insegura: {name}", "unsafe path: {name}"))
        })?;
        if is_macos_metadata(&rel) {
            continue;
        }
        let target = out.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&target).at(&target)?;
            continue;
        }
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).at(p)?;
        }
        let mut f = fs::File::create(&target).at(&target)?;
        std::io::copy(&mut entry, &mut f).at(&target)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(mode)).at(&target)?;
        }
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, out: &Path) -> Result<()> {
    let file = fs::File::open(archive).at(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    let bad = |e: std::io::Error| Error::Archive { path: archive.to_path_buf(), reason: e.to_string() };
    for entry in tar.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        if is_macos_metadata(&entry.path().map_err(bad)?) {
            continue;
        }
        // `unpack_in` rejects entries that would escape `out`.
        entry.unpack_in(out).map_err(bad)?;
    }
    Ok(())
}

/// AppleDouble files (`._*`) and `__MACOSX/` added by macOS tools.
fn is_macos_metadata(path: &Path) -> bool {
    path.components().any(|c| {
        let c = c.as_os_str().to_string_lossy();
        c.starts_with("._") || c == "__MACOSX"
    })
}

fn single_root_dir(dir: &Path) -> Result<Option<PathBuf>> {
    let entries: Vec<_> = fs::read_dir(dir).at(dir)?.collect::<std::io::Result<_>>().at(dir)?;
    match entries.as_slice() {
        [only] if only.file_type().at(only.path())?.is_dir() => Ok(Some(only.path())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Arch, Os};
    use std::io::Write;
    use std::sync::Mutex;

    fn make_tar_gz(path: &Path, root: &str) {
        let gz = flate2::write::GzEncoder::new(fs::File::create(path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let data = b"#!/bin/sh\necho php\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, format!("{root}/bin/php"), &data[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }

    fn make_zip(path: &Path) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("php.exe", opts).unwrap();
        zip.write_all(b"MZ").unwrap();
        zip.start_file("ext/php_intl.dll", opts).unwrap();
        zip.write_all(b"MZ").unwrap();
        zip.finish().unwrap();
    }

    fn sha256_of(path: &Path) -> String {
        hex::encode(Sha256::digest(fs::read(path).unwrap()))
    }

    fn package(archive: &Path, kind: ArchiveKind, sha256: String) -> Package {
        Package {
            tool: Tool::Php,
            version: "8.3.12".into(),
            os: Os::Linux,
            arch: Arch::X64,
            url: format!("file://{}", archive.display()),
            sha256,
            archive: kind,
            size: None,
        }
    }

    #[tokio::test]
    async fn installs_tar_gz_and_strips_root_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let archive = tmp.path().join("php.tar.gz");
        make_tar_gz(&archive, "php-8.3.12");

        let phases = Mutex::new(Vec::new());
        let pkg = package(&archive, ArchiveKind::TarGz, sha256_of(&archive));
        let dest = install(&paths, &pkg, |p| phases.lock().unwrap().push(p.phase)).await.unwrap();

        assert_eq!(dest, paths.bin(Tool::Php, "8.3.12"));
        assert!(dest.join("bin/php").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dest.join("bin/php")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the executable bit must be preserved");
        }
        let phases = phases.into_inner().unwrap();
        assert_eq!(phases.first(), Some(&Phase::Downloading));
        assert_eq!(phases.last(), Some(&Phase::Done));
        assert!(!paths.downloads().join("php-8.3.12.part").exists());
    }

    #[tokio::test]
    async fn ignores_appledouble_files_when_stripping_root() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let archive = tmp.path().join("php.tar.gz");
        let gz = flate2::write::GzEncoder::new(fs::File::create(&archive).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for name in ["._php-8.3.12", "php-8.3.12/php-fpm", "php-8.3.12/._php-fpm"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"x"[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();

        let pkg = package(&archive, ArchiveKind::TarGz, sha256_of(&archive));
        let dest = install(&paths, &pkg, |_| {}).await.unwrap();
        let entries: Vec<_> = fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, ["php-fpm"]);
    }

    #[tokio::test]
    async fn installs_flat_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let archive = tmp.path().join("php.zip");
        make_zip(&archive);

        let pkg = package(&archive, ArchiveKind::Zip, sha256_of(&archive));
        let dest = install(&paths, &pkg, |_| {}).await.unwrap();
        assert!(dest.join("php.exe").is_file());
        assert!(dest.join("ext/php_intl.dll").is_file());

        uninstall(&paths, Tool::Php, "8.3.12").unwrap();
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn rejects_bad_checksum_without_leaving_files() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("data"));
        let archive = tmp.path().join("php.zip");
        make_zip(&archive);

        let pkg = package(&archive, ArchiveKind::Zip, "0".repeat(64));
        let err = install(&paths, &pkg, |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::Checksum { .. }));
        assert!(!paths.bin(Tool::Php, "8.3.12").exists());
        assert!(!paths.downloads().join("php-8.3.12.part").exists());
    }
}
