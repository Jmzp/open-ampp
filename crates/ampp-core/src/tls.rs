//! Local HTTPS: a certificate authority created on this machine that signs one certificate for
//! the HTTPS projects. The CA carries name constraints, so it can only vouch for `localhost`,
//! `*.localhost` and 127.0.0.0/8: trusting it never puts real sites at risk, even if its key leaked.

use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Command;

use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use crate::error::{Error, IoContext, Result};
use crate::paths::AppPaths;

/// Common name of the CA, also how it is found in the system trust stores.
pub const CA_NAME: &str = "Open AMPP Local CA";
const CA_YEARS: i64 = 10;
/// Apple rejects TLS server certificates valid for more than 825 days.
const SERVER_DAYS: i64 = 800;
/// The server certificate is renewed when it has less than this left.
const RENEW_BEFORE_DAYS: i64 = 30;

/// Files of the server certificate, as Apache wants them.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerCert {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// What the server certificate covers and until when (`server.json`), so it is only reissued when
/// needed.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ServerMeta {
    domains: Vec<String>,
    expires: i64,
}

fn dir(paths: &AppPaths) -> PathBuf {
    paths.certs().join("local")
}

/// The CA certificate, the file users import into their trust store.
pub fn ca_cert_path(paths: &AppPaths) -> PathBuf {
    dir(paths).join("ca.pem")
}

fn ca_key_path(paths: &AppPaths) -> PathBuf {
    dir(paths).join("ca.key")
}

fn tls_error(e: rcgen::Error) -> Error {
    Error::Tls(e.to_string())
}

/// CA parameters. The issuer of the server certificate is rebuilt from them, so the subject must
/// never change.
fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, CA_NAME);
    name.push(DnType::OrganizationName, "Open AMPP");
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(NameConstraints {
        // A DNS constraint also covers every subdomain: `localhost` allows `blog.localhost`.
        permitted_subtrees: vec![
            GeneralSubtree::DnsName("localhost".into()),
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([127, 0, 0, 0], 8)),
        ],
        excluded_subtrees: Vec::new(),
    });
    params
}

/// Creates the CA the first time. Returns `true` if it was created now.
pub fn ensure_ca(paths: &AppPaths) -> Result<bool> {
    if ca_cert_path(paths).exists() && ca_key_path(paths).exists() {
        return Ok(false);
    }
    let key = KeyPair::generate().map_err(tls_error)?;
    let mut params = ca_params();
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365 * CA_YEARS);
    let cert = params.self_signed(&key).map_err(tls_error)?;
    write_private(&ca_key_path(paths), &key.serialize_pem())?;
    write_file(&ca_cert_path(paths), &cert.pem())?;
    // A new CA invalidates the server certificate signed by the previous one.
    let _ = fs::remove_file(dir(paths).join("server.json"));
    Ok(true)
}

/// Server certificate for `localhost`, 127.0.0.1 and `domains`, issued again when the list changes
/// or it is about to expire.
pub fn ensure_server_cert(paths: &AppPaths, domains: &[String]) -> Result<ServerCert> {
    ensure_ca(paths)?;
    let dir = dir(paths);
    let files = ServerCert { cert: dir.join("server.pem"), key: dir.join("server.key") };
    let meta_path = dir.join("server.json");

    let mut wanted: Vec<String> = std::iter::once("localhost".to_string()).chain(domains.iter().cloned()).collect();
    wanted.sort();
    wanted.dedup();
    let now = OffsetDateTime::now_utc();
    let current: Option<ServerMeta> = fs::read(&meta_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let fresh = current.is_some_and(|m| {
        m.domains == wanted && m.expires > (now + Duration::days(RENEW_BEFORE_DAYS)).unix_timestamp()
    });
    if fresh && files.cert.exists() && files.key.exists() {
        return Ok(files);
    }

    let ca_key = KeyPair::from_pem(&fs::read_to_string(ca_key_path(paths)).at(ca_key_path(paths))?).map_err(tls_error)?;
    let issuer = Issuer::new(ca_params(), ca_key);

    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "localhost");
    params.distinguished_name = name;
    params.subject_alt_names = wanted
        .iter()
        .map(|d| d.as_str().try_into().map(SanType::DnsName).map_err(tls_error))
        .chain(std::iter::once(Ok(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)))))
        .collect::<Result<_>>()?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;
    params.not_before = now - Duration::days(1);
    let expires = now + Duration::days(SERVER_DAYS);
    params.not_after = expires;

    let key = KeyPair::generate().map_err(tls_error)?;
    let cert = params.signed_by(&key, &issuer).map_err(tls_error)?;
    write_private(&files.key, &key.serialize_pem())?;
    write_file(&files.cert, &cert.pem())?;
    let meta = ServerMeta { domains: wanted, expires: expires.unix_timestamp() };
    write_file(&meta_path, &serde_json::to_string_pretty(&meta).expect("serializable"))?;
    Ok(files)
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).at(dir)?;
    }
    fs::write(path, contents).at(path)
}

/// Private keys are readable only by the user.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    write_file(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).at(path)?;
    }
    Ok(())
}

/// Whether the system trusts the CA. `None` when it cannot be told (Linux, where each distro and
/// browser keeps its own store).
pub fn is_trusted(paths: &AppPaths) -> Option<bool> {
    let ca = ca_cert_path(paths);
    if !ca.exists() {
        return Some(false);
    }
    if cfg!(target_os = "macos") {
        Some(quiet(Command::new("security").args(["verify-cert", "-p", "ssl", "-c"]).arg(&ca)))
    } else if cfg!(windows) {
        Some(quiet(Command::new("certutil").args(["-user", "-store", "Root", CA_NAME])))
    } else {
        None
    }
}

/// Adds the CA to the user's trust store. macOS asks for the user's password and Windows shows
/// a confirmation dialog; nothing needs administrator rights. On Linux it returns the commands to
/// run instead, since every option there needs `sudo`.
pub fn trust(paths: &AppPaths) -> Result<()> {
    ensure_ca(paths)?;
    let ca = ca_cert_path(paths);
    let ok = if cfg!(target_os = "macos") {
        let keychain = dirs_home().join("Library/Keychains/login.keychain-db");
        quiet(Command::new("security").args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl", "-k"]).arg(&keychain).arg(&ca))
    } else if cfg!(windows) {
        quiet(Command::new("certutil").args(["-user", "-addstore", "Root"]).arg(&ca))
    } else {
        return Err(Error::Tls(linux_trust_commands(&ca)));
    };
    if ok {
        Ok(())
    } else {
        Err(Error::Tls(tr!(
            "no se pudo agregar la CA al almacén de confianza (¿se canceló el diálogo?)",
            "could not add the CA to the trust store (was the dialog cancelled?)"
        )))
    }
}

/// Commands to trust the CA on Linux: the system store and the one Chrome and Firefox use (NSS).
pub fn linux_trust_commands(ca: &Path) -> String {
    let ca = ca.display();
    format!(
        "# Debian / Ubuntu\nsudo cp \"{ca}\" /usr/local/share/ca-certificates/open-ampp.crt && sudo update-ca-certificates\n\
         # Fedora / RHEL\nsudo cp \"{ca}\" /etc/pki/ca-trust/source/anchors/open-ampp.pem && sudo update-ca-trust\n\
         # Chrome and Firefox (NSS, needs libnss3-tools / nss-tools)\n\
         certutil -d sql:$HOME/.pki/nssdb -A -t \"C,,\" -n \"{CA_NAME}\" -i \"{ca}\""
    )
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Runs a command without a console window or output; `true` if it succeeded.
fn quiet(cmd: &mut Command) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_is_created_once_and_the_server_cert_follows_the_domains() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        assert!(ensure_ca(&paths).unwrap());
        let ca = fs::read_to_string(ca_cert_path(&paths)).unwrap();
        assert!(ca.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(!ensure_ca(&paths).unwrap(), "an existing CA is kept");
        assert_eq!(fs::read_to_string(ca_cert_path(&paths)).unwrap(), ca);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(ca_key_path(&paths)).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let domains = vec!["blog.localhost".to_string()];
        let files = ensure_server_cert(&paths, &domains).unwrap();
        let first = fs::read_to_string(&files.cert).unwrap();
        let meta: ServerMeta = serde_json::from_slice(&fs::read(dir(&paths).join("server.json")).unwrap()).unwrap();
        assert_eq!(meta.domains, ["blog.localhost", "localhost"]);
        let days_left = (meta.expires - OffsetDateTime::now_utc().unix_timestamp()) / 86_400;
        assert!((SERVER_DAYS - 1..=SERVER_DAYS).contains(&days_left), "{days_left}");

        assert_eq!(fs::read_to_string(&ensure_server_cert(&paths, &domains).unwrap().cert).unwrap(), first, "unchanged");
        let more = vec!["shop.localhost".to_string(), "blog.localhost".to_string()];
        assert_ne!(fs::read_to_string(&ensure_server_cert(&paths, &more).unwrap().cert).unwrap(), first, "reissued");
    }

    #[test]
    fn server_cert_is_renewed_before_it_expires() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path());
        let files = ensure_server_cert(&paths, &[]).unwrap();
        let first = fs::read_to_string(&files.cert).unwrap();
        let meta = ServerMeta {
            domains: vec!["localhost".into()],
            expires: (OffsetDateTime::now_utc() + Duration::days(5)).unix_timestamp(),
        };
        fs::write(dir(&paths).join("server.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        assert_ne!(fs::read_to_string(&ensure_server_cert(&paths, &[]).unwrap().cert).unwrap(), first);
    }

    #[test]
    fn linux_commands_point_at_the_ca() {
        let text = linux_trust_commands(Path::new("/data/certs/local/ca.pem"));
        assert!(text.contains("update-ca-certificates") && text.contains("update-ca-trust") && text.contains("nssdb"));
        assert_eq!(text.matches("\"/data/certs/local/ca.pem\"").count(), 3);
    }
}
