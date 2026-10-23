#!/usr/bin/env bash
# Builds a portable (relocatable) MariaDB for macOS or Linux.
# Usage: WORK=/tmp/build ./build-mariadb.sh <version> <output-dir>
# Produces: <output>/mariadb-<version>-<os>-<arch>.tar.gz
# Requires: cmake ≥ 3.23, bison ≥ 3, a C/C++ compiler and (on Linux) ncurses headers.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=${1:?usage: build-mariadb.sh <version> <output>}
OUT=$(mkdir -p "${2:?usage: build-mariadb.sh <version> <output>}" && cd "$2" && pwd)
WORK=${WORK:-$PWD/build-work}
mkdir -p "$WORK" && WORK=$(cd "$WORK" && pwd)
source "$HERE/sources.sh"
source "$HERE/lib.sh"

SHA=$(mariadb_sha256 "$VERSION") || die "no checksum in sources.sh for MariaDB $VERSION"

# CI sets SKIP_EXISTING=1 so adding a version does not rebuild the published ones. Uses the API
# through curl because the Linux builds run in a container without `gh`.
asset="mariadb-$VERSION-$OS-$ARCH.tar.gz"
if [ "${SKIP_EXISTING:-}" = 1 ] && curl -fsSL ${GH_TOKEN:+-H "Authorization: Bearer $GH_TOKEN"} \
  "https://api.github.com/repos/${GITHUB_REPOSITORY:-Jmzp/open-ampp}/releases/tags/bin-mariadb-$VERSION" 2>/dev/null \
  | grep -q "\"name\": *\"$asset\""; then
  log "$asset is already published; skipping"
  exit 0
fi
command -v cmake >/dev/null || die "cmake is missing"
bison --version | head -1 | grep -qE ' [3-9]\.' || die "bison ≥ 3 is required (found: $(bison --version | head -1))"

PREFIX="$WORK/mariadb-$VERSION"
BUILD="$WORK/build-mariadb-$VERSION"
DEPS="$WORK/deps-mariadb"   # static dependencies, not packaged
rm -rf "$PREFIX"
# KEEP_BUILD=1 reuses the previous build tree (handy locally while tweaking flags).
[ "${KEEP_BUILD:-0}" = 1 ] || rm -rf "$BUILD"

# Static OpenSSL for server and client: with WITH_SSL=bundled, Connector/C insists on GnuTLS.
build_openssl "$DEPS"

src=$(fetch "$(mariadb_url "$VERSION")" "$SHA" "$WORK/src")

# Engines and plugins that are heavy or need external libraries and add nothing for local development.
disabled_plugins=(
  ROCKSDB MROONGA SPIDER CONNECT COLUMNSTORE OQGRAPH S3 SPHINX TOKUDB DUCKDB
  HASHICORP_KEY_MANAGEMENT AUTH_GSSAPI AUTH_PAM AUTH_PAM_V1
  PROVIDER_BZIP2 PROVIDER_LZ4 PROVIDER_LZMA PROVIDER_LZO PROVIDER_SNAPPY PROVIDER_ZSTD
)
plugin_flags=()
for p in "${disabled_plugins[@]}"; do plugin_flags+=("-DPLUGIN_$p=NO"); done
# Connector/C plugins take OFF instead of NO.
plugin_flags+=("-DCLIENT_PLUGIN_AUTH_GSSAPI_CLIENT=OFF" "-DCLIENT_PLUGIN_ZSTD=OFF")

# pkg-config only sees our dependencies, so no Homebrew or other system libraries sneak in.
export PKG_CONFIG_PATH="" PKG_CONFIG_LIBDIR="$DEPS/lib/pkgconfig"

linker_flags=()
# On glibc < 2.34 pthread is a separate library, and the static OpenSSL needs it.
[ "$OS" = linux ] && linker_flags=(-DCMAKE_EXE_LINKER_FLAGS=-pthread -DCMAKE_SHARED_LINKER_FLAGS=-pthread -DCMAKE_MODULE_LINKER_FLAGS=-pthread)

log "configuring MariaDB $VERSION"
cmake -S "$src" -B "$BUILD" \
  -DBUILD_CONFIG=mysql_release \
  -DCMAKE_IGNORE_PREFIX_PATH="/opt/homebrew;/usr/local" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DINSTALL_LAYOUT=STANDALONE \
  -DWITH_SSL="$DEPS" \
  -DOPENSSL_ROOT_DIR="$DEPS" \
  -DOPENSSL_USE_STATIC_LIBS=ON \
  -DWITH_ZLIB=bundled \
  -DWITH_PCRE=bundled \
  -DWITH_LIBFMT=bundled \
  -DWITH_SYSTEMD=no \
  -DWITH_WSREP=OFF \
  -DWITH_UNIT_TESTS=OFF \
  -DWITH_EMBEDDED_SERVER=OFF \
  -DWITH_MARIABACKUP=OFF \
  -DWITH_JEMALLOC=no \
  -DWITH_INNODB_BZIP2=OFF -DWITH_INNODB_LZ4=OFF -DWITH_INNODB_LZMA=OFF -DWITH_INNODB_LZO=OFF -DWITH_INNODB_SNAPPY=OFF \
  -DWITH_ROCKSDB_LZ4=OFF -DWITH_ROCKSDB_ZSTD=OFF \
  -DIGNORE_AIO_CHECK=ON -DWITH_URING=no \
  -DINSTALL_MYSQLTESTDIR= -DINSTALL_SQLBENCHDIR= \
  "${plugin_flags[@]}" ${linker_flags[@]+"${linker_flags[@]}"} >"$WORK/mariadb-cmake.log" 2>&1 || { tail -40 "$WORK/mariadb-cmake.log"; die "cmake failed"; }

log "compiling (this takes a while)"
cmake --build "$BUILD" --parallel "$JOBS" >"$WORK/mariadb-build.log" 2>&1 || { tail -60 "$WORK/mariadb-build.log"; die "build failed"; }
cmake --install "$BUILD" >/dev/null

log "cleaning up, stripping and relocating"
rm -rf "$PREFIX"/{mysql-test,sql-bench,man,include,docs,support-files/rpm}
find "$PREFIX/lib" \( -name '*.a' -o -name '*.la' \) -delete
rm -rf "$PREFIX/lib/pkgconfig"
binaries_in "$PREFIX" | while read -r f; do
  chmod u+w "$f"
  if [ "$OS" = macos ]; then strip -S -x "$f" 2>/dev/null || true; else strip --strip-unneeded "$f" 2>/dev/null || true; fi
done
relocate "$PREFIX"
verify_relocatable "$PREFIX"
"$PREFIX/bin/mariadbd" --version

package "$PREFIX" "mariadb-$VERSION" "$OUT/mariadb-$VERSION-$OS-$ARCH.tar.gz"
