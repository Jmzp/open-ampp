#!/usr/bin/env bash
# Compila Apache httpd portable (relocalizable) para macOS o Linux.
# Uso: WORK=/tmp/build ./build-apache.sh <directorio-de-salida>
# Produce: <salida>/apache-<versión>-<os>-<arch>.tar.gz
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$(mkdir -p "${1:?uso: build-apache.sh <salida>}" && cd "$1" && pwd)
WORK=${WORK:-$PWD/build-work}
mkdir -p "$WORK" && WORK=$(cd "$WORK" && pwd)
source "$HERE/sources.sh"
source "$HERE/lib.sh"

SRC="$WORK/src"
DEPS="$WORK/deps-apache"     # dependencias estáticas, no se empaquetan
PREFIX="$WORK/apache-$HTTPD_VERSION"
rm -rf "$DEPS" "$PREFIX"
mkdir -p "$SRC" "$DEPS"

build_openssl "$DEPS"

log "PCRE2 $PCRE2_VERSION (estática)"
dir=$(fetch "$PCRE2_URL" "$PCRE2_SHA256" "$SRC")
(cd "$dir" && ./configure --prefix="$DEPS" --disable-shared --enable-static --with-pic >/dev/null \
  && make -j"$JOBS" >/dev/null && make install >/dev/null)

log "expat $EXPAT_VERSION (estática)"
dir=$(fetch "$EXPAT_URL" "$EXPAT_SHA256" "$SRC")
(cd "$dir" && ./configure --prefix="$DEPS" --disable-shared --enable-static --with-pic \
  --without-docbook --without-examples --without-tests >/dev/null \
  && make -j"$JOBS" >/dev/null && make install >/dev/null)

log "httpd $HTTPD_VERSION + APR $APR_VERSION + APR-util $APRUTIL_VERSION"
httpd=$(fetch "$HTTPD_URL" "$HTTPD_SHA256" "$SRC")
apr=$(fetch "$APR_URL" "$APR_SHA256" "$SRC")
aprutil=$(fetch "$APRUTIL_URL" "$APRUTIL_SHA256" "$SRC")
rm -rf "$httpd/srclib/apr" "$httpd/srclib/apr-util"
cp -a "$apr" "$httpd/srclib/apr"
cp -a "$aprutil" "$httpd/srclib/apr-util"

extra_libs=""
[ "$OS" = linux ] && extra_libs="-ldl -pthread"
(
  cd "$httpd"
  # pkg-config solo ve nuestras dependencias: así no se cuelan librerías de Homebrew u otras del sistema.
  export PKG_CONFIG_PATH="" PKG_CONFIG_LIBDIR="$DEPS/lib/pkgconfig"
  # Módulos que requieren librerías externas (brotli, nghttp2, lua, curl/jansson) y no aportan en local.
  CFLAGS="-O2 -fPIC" LIBS="$extra_libs" ./configure \
    --disable-brotli --disable-http2 --disable-proxy-http2 --disable-lua --disable-md \
    --prefix="$PREFIX" \
    --with-included-apr \
    --with-pcre="$DEPS/bin/pcre2-config" \
    --with-expat="$DEPS" \
    --with-ssl="$DEPS" \
    --enable-ssl \
    --enable-so \
    --enable-mods-shared=all \
    --enable-mpms-shared=all \
    --with-mpm=event >/dev/null
  make -j"$JOBS" >/dev/null
  make install >/dev/null
)

log "limpiando y relocalizando"
rm -rf "$PREFIX"/{manual,man,build,include,cgi-bin,htdocs}
rm -f "$PREFIX"/lib/*.a "$PREFIX"/lib/*.la "$PREFIX"/lib/*.exp
rm -rf "$PREFIX"/lib/pkgconfig
relocate "$PREFIX"
verify_relocatable "$PREFIX"
"$PREFIX/bin/httpd" -v

package "$PREFIX" "apache-$HTTPD_VERSION" "$OUT/apache-$HTTPD_VERSION-$OS-$ARCH.tar.gz"
