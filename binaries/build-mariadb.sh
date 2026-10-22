#!/usr/bin/env bash
# Compila MariaDB portable (relocalizable) para macOS o Linux.
# Uso: WORK=/tmp/build ./build-mariadb.sh <versión> <directorio-de-salida>
# Produce: <salida>/mariadb-<versión>-<os>-<arch>.tar.gz
# Requiere: cmake ≥ 3.23, bison ≥ 3, compilador C/C++ y (en Linux) libncurses-dev.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=${1:?uso: build-mariadb.sh <versión> <salida>}
OUT=$(mkdir -p "${2:?uso: build-mariadb.sh <versión> <salida>}" && cd "$2" && pwd)
WORK=${WORK:-$PWD/build-work}
mkdir -p "$WORK" && WORK=$(cd "$WORK" && pwd)
source "$HERE/sources.sh"
source "$HERE/lib.sh"

SHA=$(mariadb_sha256 "$VERSION") || die "versión de MariaDB sin checksum en sources.sh: $VERSION"
command -v cmake >/dev/null || die "falta cmake"
bison --version | head -1 | grep -qE ' [3-9]\.' || die "se necesita bison ≥ 3 (hay: $(bison --version | head -1))"

PREFIX="$WORK/mariadb-$VERSION"
BUILD="$WORK/build-mariadb-$VERSION"
DEPS="$WORK/deps-mariadb"   # dependencias estáticas, no se empaquetan
rm -rf "$PREFIX"
# KEEP_BUILD=1 reutiliza la compilación anterior (útil en local al ajustar flags).
[ "${KEEP_BUILD:-0}" = 1 ] || rm -rf "$BUILD"

# OpenSSL estática para servidor y cliente: con WITH_SSL=bundled el Connector/C exige GnuTLS.
build_openssl "$DEPS"

src=$(fetch "$(mariadb_url "$VERSION")" "$SHA" "$WORK/src")

# Motores y plugins con dependencias externas o pesados que no aportan en desarrollo local.
disabled_plugins=(
  ROCKSDB MROONGA SPIDER CONNECT COLUMNSTORE OQGRAPH S3 SPHINX TOKUDB DUCKDB
  HASHICORP_KEY_MANAGEMENT AUTH_GSSAPI AUTH_PAM AUTH_PAM_V1
  PROVIDER_BZIP2 PROVIDER_LZ4 PROVIDER_LZMA PROVIDER_LZO PROVIDER_SNAPPY PROVIDER_ZSTD
)
plugin_flags=()
for p in "${disabled_plugins[@]}"; do plugin_flags+=("-DPLUGIN_$p=NO"); done
# Los plugins del Connector/C usan OFF en lugar de NO.
plugin_flags+=("-DCLIENT_PLUGIN_AUTH_GSSAPI_CLIENT=OFF" "-DCLIENT_PLUGIN_ZSTD=OFF")

# pkg-config solo ve nuestras dependencias: así no se cuelan librerías de Homebrew u otras del sistema.
export PKG_CONFIG_PATH="" PKG_CONFIG_LIBDIR="$DEPS/lib/pkgconfig"

linker_flags=()
# En glibc < 2.34 pthread es una librería aparte y la OpenSSL estática la necesita.
[ "$OS" = linux ] && linker_flags=(-DCMAKE_EXE_LINKER_FLAGS=-pthread -DCMAKE_SHARED_LINKER_FLAGS=-pthread -DCMAKE_MODULE_LINKER_FLAGS=-pthread)

log "configurando MariaDB $VERSION"
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
  "${plugin_flags[@]}" ${linker_flags[@]+"${linker_flags[@]}"} >"$WORK/mariadb-cmake.log" 2>&1 || { tail -40 "$WORK/mariadb-cmake.log"; die "cmake falló"; }

log "compilando (puede tardar bastante)"
cmake --build "$BUILD" --parallel "$JOBS" >"$WORK/mariadb-build.log" 2>&1 || { tail -60 "$WORK/mariadb-build.log"; die "compilación falló"; }
cmake --install "$BUILD" >/dev/null

log "limpiando, quitando símbolos y relocalizando"
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
