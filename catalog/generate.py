#!/usr/bin/env python3
"""Generates catalog/manifest.json from the official sources.

Usage: python3 catalog/generate.py

- Our own builds (the repo's `bin-*` releases, see .github/workflows/binaries.yml) come
  first: GitHub reports each asset's SHA-256 in its `digest` field. They cover macOS and Linux.
- Windows uses the official builds (php.net, mariadb.org, Apache Lounge) with the checksums
  those projects publish.
- Helper tools (phpMyAdmin, Composer) also use their published checksums.
"""

import json
import os
import re
import urllib.error
import urllib.request
from pathlib import Path

MANIFEST = Path(__file__).with_name("manifest.json")
UA = {"User-Agent": "Mozilla/5.0 (open-ampp catalog generator)"}
REPO = "Jmzp/open-ampp"

PHP_MINORS = ["8.2", "8.3", "8.4", "8.5"]
# End-of-life PHP 7 releases for old projects, Windows only: static-php-cli (our macOS and Linux
# builds) does not support PHP 7. They are final, so the archived php.net zips are pinned with
# the SHA-256 that php.net published (its sha256sum.txt no longer lists them). php.net archived
# only the thread-safe builds of these versions, which serve FastCGI just the same.
PHP_LEGACY_WINDOWS = [
    ("7.2.34", "php-7.2.34-Win32-VC15-x64.zip", "3a3d026aacf8da57ff01ace2f78f8f57494744bfb669f8545cc8f9c4ff0450a5"),
    ("7.4.33", "php-7.4.33-Win32-vc15-x64.zip", "cdbb85b45f38f282f05764ca08648b5f92db99c75b2fb3848eb4a559f6553b48"),
]
MARIADB_LTS = ["10.11", "11.4", "11.8", "12.3"]


def get(url):
    headers = dict(UA)
    # Authenticated GitHub API calls avoid the 60 requests/hour anonymous limit.
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as r:
        return r.read().decode("utf-8", "replace")


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
    base = "https://downloads.php.net/~windows/releases/"
    # Rebuilds of a release carry a build number (php-8.4.27-1-nts-...) and replace the
    # original zip, which then 404s; the highest build of each version wins.
    rows = re.findall(r"([0-9a-f]{64}) \*(php-([\d.]+)(?:-(\d+))?-nts-Win32-vs1[67]-x64\.zip)",
                      get(base + "sha256sum.txt"))
    by_version, build = {}, {}
    for sha, name, v, b in rows:
        if v not in build or int(b or 0) > build[v]:
            by_version[v], build[v] = (sha, name), int(b or 0)
    if not by_version:
        # Fail loudly instead of publishing a catalog without PHP for Windows.
        raise SystemExit("php.net sha256sum.txt: no Windows x64 NTS builds matched")
    for v in latest_per_minor(by_version, PHP_MINORS):
        sha, name = by_version[v]
        yield pkg("php", v, "windows", "x64", base + name, sha, "zip")
    for v, name, sha in PHP_LEGACY_WINDOWS:
        yield pkg("php", v, "windows", "x64", base + "archives/" + name, sha, "zip")


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


def own_releases():
    try:
        releases = json.loads(get(f"https://api.github.com/repos/{REPO}/releases?per_page=100"))
    except urllib.error.HTTPError as e:
        print(f"  no own releases ({e.code}); third-party sources only")
        return
    pat = re.compile(r"(apache|mariadb|php)-([\d.]+)-(macos|linux)-(x64|arm64)\.tar\.gz")
    for release in releases:
        if not release["tag_name"].startswith("bin-"):
            continue
        for asset in release["assets"]:
            m = pat.fullmatch(asset["name"])
            digest = asset.get("digest") or ""
            if m and digest.startswith("sha256:"):
                tool, version, os, arch = m.groups()
                yield pkg(tool, version, os, arch, asset["browser_download_url"],
                          digest.removeprefix("sha256:"), "tar-gz", asset["size"])


def phpmyadmin():
    """Latest phpMyAdmin release (all languages) with its published SHA-256."""
    version = json.loads(get("https://www.phpmyadmin.net/home_page/version.json"))["version"]
    url = f"https://files.phpmyadmin.net/phpMyAdmin/{version}/phpMyAdmin-{version}-all-languages.zip"
    sha = get(url + ".sha256").split()[0]
    return {"name": "phpmyadmin", "version": version, "url": url, "sha256": sha, "archive": "zip", "size": None}


def composer():
    """Latest stable Composer 2 with its official checksum."""
    stable = json.loads(get("https://getcomposer.org/versions"))["stable"][0]
    url = "https://getcomposer.org" + stable["path"]
    sha = get(url + ".sha256sum").split()[0]
    return {"name": "composer", "version": stable["version"], "url": url, "sha256": sha, "size": None}


def minor(version):
    return ".".join(version.split(".")[:2])


def main():
    # Only the newest patch of each series: older ones may carry known vulnerabilities.
    newest = {}
    for p in own_releases():
        key = (p["tool"], p["os"], p["arch"], minor(p["version"]))
        if key not in newest or vkey(p["version"]) > vkey(newest[key]["version"]):
            newest[key] = p
    packages = list(newest.values())
    # A third-party package is only added when we have no build of our own for that tool,
    # platform and minor version.
    covered = {(p["tool"], p["os"], p["arch"], minor(p["version"])) for p in packages}
    for source in (php_windows(), mariadb(), apache_windows()):
        for p in source:
            if (p["tool"], p["os"], p["arch"], minor(p["version"])) not in covered:
                packages.append(p)
    packages.sort(key=lambda p: (p["tool"], p["os"], p["arch"], vkey(p["version"])))

    extras = [e for e in (phpmyadmin(), composer()) if e]

    MANIFEST.write_text(json.dumps({"schema": 1, "packages": packages, "extras": extras}, indent=2) + "\n")
    print(f"wrote {len(packages)} packages and {len(extras)} tools to {MANIFEST}")


if __name__ == "__main__":
    main()
