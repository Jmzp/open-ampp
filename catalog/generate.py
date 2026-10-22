#!/usr/bin/env python3
"""Genera catalog/manifest.json a partir de las fuentes oficiales.

Uso: python3 catalog/generate.py

- Si la fuente publica checksums (php.net, mariadb.org, Apache Lounge) se usan tal cual.
- Si no (static-php-cli), se descarga el archivo y se calcula el SHA-256. Los hashes
  ya presentes en manifest.json para la misma URL se reutilizan para no re-descargar.
"""

import hashlib
import json
import re
import urllib.request
from pathlib import Path

MANIFEST = Path(__file__).with_name("manifest.json")
UA = {"User-Agent": "Mozilla/5.0 (open-ampp catalog generator)"}

PHP_MINORS = ["8.2", "8.3", "8.4"]
MARIADB_LTS = ["10.11", "11.4", "11.8"]


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
        return r.read().decode("utf-8", "replace")


def sha256_of_url(url):
    h = hashlib.sha256()
    size = 0
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=300) as r:
        while chunk := r.read(1 << 20):
            h.update(chunk)
            size += len(chunk)
    return h.hexdigest(), size


def vkey(v):
    return tuple(int(p) for p in v.split("."))


def latest_per_minor(versions, minors):
    out = {}
    for v in versions:
        minor = ".".join(v.split(".")[:2])
        if minor in minors and (minor not in out or vkey(v) > vkey(out[minor])):
            out[minor] = v
    return [out[m] for m in minors if m in out]


def pkg(tool, version, os, arch, url, sha256, archive, size=None):
    return {
        "tool": tool, "version": version, "os": os, "arch": arch,
        "url": url, "sha256": sha256.lower(), "archive": archive, "size": size,
    }


def php_windows():
    base = "https://windows.php.net/downloads/releases/"
    rows = re.findall(r"([0-9a-f]{64}) \*(php-([\d.]+)-nts-Win32-vs1[67]-x64\.zip)", get(base + "sha256sum.txt"))
    by_version = {v: (sha, name) for sha, name, v in rows}
    for v in latest_per_minor(by_version, PHP_MINORS):
        sha, name = by_version[v]
        yield pkg("php", v, "windows", "x64", base + name, sha, "zip")


def php_static(known):
    base = "https://dl.static-php.dev/static-php-cli/common/"
    names = set(re.findall(r"php-[\d.]+-fpm-(?:linux|macos)-(?:x86_64|aarch64)\.tar\.gz", get(base)))
    for os_name, os in (("linux", "linux"), ("macos", "macos")):
        for march, arch in (("x86_64", "x64"), ("aarch64", "arm64")):
            pat = re.compile(rf"php-([\d.]+)-fpm-{os_name}-{march}\.tar\.gz")
            versions = {m.group(1): n for n in names if (m := pat.fullmatch(n))}
            for v in latest_per_minor(versions, PHP_MINORS):
                url = base + versions[v]
                if url in known:
                    sha, size = known[url]
                else:
                    print(f"  descargando {versions[v]} para calcular sha256…")
                    sha, size = sha256_of_url(url)
                yield pkg("php", v, os, arch, url, sha, "tar-gz", size)


def mariadb():
    base = "https://archive.mariadb.org/"
    releases = re.findall(r"mariadb-(\d+\.\d+\.\d+)/", get(base))
    for v in latest_per_minor(releases, MARIADB_LTS):
        for folder, os, archive, suffix in (
            ("bintar-linux-systemd-x86_64", "linux", "tar-gz", "linux-systemd-x86_64.tar.gz"),
            ("winx64-packages", "windows", "zip", "winx64.zip"),
        ):
            name = f"mariadb-{v}-{suffix}"
            sums = get(f"{base}mariadb-{v}/{folder}/sha256sums.txt")
            m = re.search(rf"([0-9a-f]{{64}})\s+(?:\./)?{re.escape(name)}$", sums, re.M)
            if m:
                yield pkg("mariadb", v, os, "x64", f"{base}mariadb-{v}/{folder}/{name}", m.group(1), archive)


def apache_windows():
    page = get("https://www.apachelounge.com/download/")
    path = re.search(r'href="(/download/[^"]+/httpd-([\d.]+)-\d+-Win64-VS\d+\.zip)"', page)
    if not path:
        return
    url = "https://www.apachelounge.com" + path.group(1)
    sums = get(url + ".txt")
    m = re.search(r"SHA256-Checksum for: [^\n]+\n([0-9A-Fa-f]{64})", sums)
    if m:
        yield pkg("apache", path.group(2), "windows", "x64", url, m.group(1), "zip")


def main():
    known = {}
    if MANIFEST.exists():
        for p in json.loads(MANIFEST.read_text()).get("packages", []):
            known[p["url"]] = (p["sha256"], p.get("size"))

    packages = []
    for source in (php_windows(), php_static(known), mariadb(), apache_windows()):
        packages.extend(source)
    packages.sort(key=lambda p: (p["tool"], p["os"], p["arch"], vkey(p["version"])))

    MANIFEST.write_text(json.dumps({"schema": 1, "packages": packages}, indent=2) + "\n")
    print(f"{len(packages)} paquetes escritos en {MANIFEST}")


if __name__ == "__main__":
    main()
