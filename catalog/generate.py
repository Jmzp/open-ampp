#!/usr/bin/env python3
"""Generates catalog/manifest.json from the official sources.

Usage: python3 catalog/generate.py

- Our own builds (the repo's `bin-*` releases, see .github/workflows/binaries.yml) come
  first: GitHub reports each asset's SHA-256 in its `digest` field. They cover macOS and Linux.
- Windows uses the official builds (php.net, mariadb.org, Apache Lounge) with the checksums
  those projects publish.
- Helper tools (phpMyAdmin, Composer) also use their published checksums; Mailpit, which has
  one build per platform, goes in `native_extras` with the digests GitHub reports.
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
    rows = re.findall(r"([0-9a-f]{64}) \*(php-([\d.]+)-nts-Win32-vs1[67]-x64\.zip)", get(base + "sha256sum.txt"))
    by_version = {v: (sha, name) for sha, name, v in rows}
    for v in latest_per_minor(by_version, PHP_MINORS):
        sha, name = by_version[v]
        yield pkg("php", v, "windows", "x64", base + name, sha, "zip")


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


MAILPIT_ASSETS = {
    "mailpit-darwin-arm64.tar.gz": ("macos", "arm64", "tar-gz"),
    "mailpit-darwin-amd64.tar.gz": ("macos", "x64", "tar-gz"),
    "mailpit-linux-arm64.tar.gz": ("linux", "arm64", "tar-gz"),
    "mailpit-linux-amd64.tar.gz": ("linux", "x64", "tar-gz"),
    "mailpit-windows-amd64.zip": ("windows", "x64", "zip"),
}


def mailpit():
    """Latest Mailpit release, one entry per platform, with GitHub's SHA-256 digests."""
    release = json.loads(get("https://api.github.com/repos/axllent/mailpit/releases/latest"))
    version = release["tag_name"].removeprefix("v")
    for asset in release["assets"]:
        target = MAILPIT_ASSETS.get(asset["name"])
        digest = asset.get("digest") or ""
        if target and digest.startswith("sha256:"):
            os_, arch, archive = target
            yield {
                "name": "mailpit", "version": version, "url": asset["browser_download_url"],
                "sha256": digest.removeprefix("sha256:"), "archive": archive, "size": asset["size"],
                "os": os_, "arch": arch,
            }


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
    # Builds up to 0.2.0 reject the whole catalog on an unknown name in `extras`, so
    # per-platform tools live in a key of their own that they ignore.
    native = sorted(mailpit(), key=lambda e: (e["name"], e["os"], e["arch"]))

    manifest = {"schema": 1, "packages": packages, "extras": extras, "native_extras": native}
    MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {len(packages)} packages and {len(extras) + len(native)} tools to {MANIFEST}")


if __name__ == "__main__":
    main()
