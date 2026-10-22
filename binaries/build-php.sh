#!/usr/bin/env bash
# Builds a static PHP (CLI + FPM) with static-php-cli and the extensions from sources.sh.
# Usage: WORK=/tmp/build ./build-php.sh <version|minor> <output-dir>
#   A minor version (`8.4`) resolves to its latest patch release on php.net, so scheduled
#   builds always pick up security fixes.
# Produces: <output>/php-<version>-<os>-<arch>.tar.gz containing php-<version>/{php,php-fpm}
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=${1:?usage: build-php.sh <version> <output>}
OUT=$(mkdir -p "${2:?usage: build-php.sh <version> <output>}" && cd "$2" && pwd)
WORK=${WORK:-$PWD/build-work}
mkdir -p "$WORK" && WORK=$(cd "$WORK" && pwd)
source "$HERE/sources.sh"
source "$HERE/lib.sh"

if [[ $VERSION =~ ^[0-9]+\.[0-9]+$ ]]; then
  minor=$VERSION
  VERSION=$(curl -fsSL "https://www.php.net/releases/index.php?json&version=$minor" \
    | sed -n 's/.*"version": *"\([0-9.]*\)".*/\1/p')
  [ -n "$VERSION" ] || die "could not resolve the latest PHP $minor release"
  log "PHP $minor → $VERSION"
fi

# Scheduled CI runs set SKIP_EXISTING=1: nothing to do when this exact build is already published.
asset="php-$VERSION-$OS-$ARCH.tar.gz"
if [ "${SKIP_EXISTING:-}" = 1 ] && gh release view "bin-php-$VERSION" --json assets --jq '.assets[].name' 2>/dev/null | grep -qx "$asset"; then
  log "$asset is already published; skipping"
  exit 0
fi

case "$ARCH" in x64) march=x86_64 ;; arm64) march=aarch64 ;; esac
platform="$OS-$march"

# One spc working directory per PHP version: it caches downloaded sources between runs.
SPC_DIR="$WORK/spc-$VERSION"
mkdir -p "$SPC_DIR"
if [ ! -x "$SPC_DIR/spc" ]; then
  log "static-php-cli $SPC_VERSION ($platform)"
  # The tarball holds the single `spc` executable.
  fetch "$(spc_url "$platform")" "$(spc_sha256 "$platform")" "$SPC_DIR" >/dev/null
fi

cd "$SPC_DIR"
log "PHP $VERSION: checking the toolchain"
./spc doctor --auto-fix
log "PHP $VERSION: downloading sources"
# spc reads release metadata from the GitHub API, which rejects bursts of requests (e.g. many CI
# jobs starting together), so retry with a growing pause.
for attempt in 1 2 3 4 5; do
  ./spc download --with-php="$VERSION" --for-extensions="$PHP_EXTENSIONS" --for-libs="$PHP_EXTRA_LIBS" \
    --prefer-pre-built --retry=3 && break
  [ "$attempt" = 5 ] && die "could not download the PHP $VERSION sources"
  log "download failed (attempt $attempt); retrying in $((attempt * 45))s"
  sleep $((attempt * 45))
done
log "PHP $VERSION: building (this takes a while)"
./spc build "$PHP_EXTENSIONS" --with-libs="$PHP_EXTRA_LIBS" --build-cli --build-fpm

PREFIX="$WORK/php-$VERSION-$OS-$ARCH"
rm -rf "$PREFIX" && mkdir -p "$PREFIX"
cp buildroot/bin/php "$PREFIX/php"
cp "$(ls buildroot/bin/php-fpm buildroot/sbin/php-fpm 2>/dev/null | head -1)" "$PREFIX/php-fpm"
if [ "$OS" = macos ]; then
  # Apple Silicon refuses to run unsigned binaries; an ad-hoc signature is enough.
  for b in php php-fpm; do codesign --force --sign - "$PREFIX/$b"; done
fi
verify_relocatable "$PREFIX"

# Fail early if an extension we rely on did not make it into the binary.
modules=$("$PREFIX/php" -n -m | tr 'A-Z' 'a-z')
for ext in mysqli pdo_mysql mbstring intl gd zip openssl curl sodium; do
  grep -qx "$ext" <<<"$modules" || die "PHP $VERSION was built without $ext"
done
grep -qx "zend opcache" <<<"$modules" || die "PHP $VERSION was built without opcache"
"$PREFIX/php" -n -v

package "$PREFIX" "php-$VERSION" "$OUT/php-$VERSION-$OS-$ARCH.tar.gz"
