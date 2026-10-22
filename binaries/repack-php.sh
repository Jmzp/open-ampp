#!/usr/bin/env bash
# Une los binarios estáticos de static-php-cli (php CLI + php-fpm) en un solo paquete.
# Uso: WORK=/tmp/build ./repack-php.sh <versión> <directorio-de-salida> [os] [arch]
# Produce: <salida>/php-<versión>-<os>-<arch>.tar.gz con php-<versión>/{php,php-fpm}
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
VERSION=${1:?uso: repack-php.sh <versión> <salida> [os] [arch]}
OUT=$(mkdir -p "${2:?uso: repack-php.sh <versión> <salida>}" && cd "$2" && pwd)
WORK=${WORK:-$PWD/build-work}
mkdir -p "$WORK" && WORK=$(cd "$WORK" && pwd)
source "$HERE/lib.sh"
# Los binarios son estáticos: se pueden empaquetar para otra plataforma desde cualquier máquina.
OS=${3:-$OS}
ARCH=${4:-$ARCH}
case "$ARCH" in x64) march=x86_64 ;; arm64) march=aarch64 ;; esac

BASE=https://dl.static-php.dev/static-php-cli/common
PREFIX="$WORK/php-$VERSION-$OS-$ARCH"
rm -rf "$PREFIX" && mkdir -p "$PREFIX" "$WORK/downloads"

for sapi in cli fpm; do
  name="php-$VERSION-$sapi-$OS-$march.tar.gz"
  log "descargando $name"
  # static-php-cli no publica checksums: el paquete final lleva el nuestro en el catálogo.
  curl -fsSL --retry 3 -o "$WORK/downloads/$name" "$BASE/$name"
  tar -xzf "$WORK/downloads/$name" -C "$PREFIX"
done
[ -x "$PREFIX/php" ] && [ -x "$PREFIX/php-fpm" ] || die "faltan binarios en $PREFIX"
if [ "$OS" = macos ] && [ "$(uname -s)" = Darwin ]; then
  # Los binarios descargados no llevan firma ad-hoc en todos los casos; en arm64 es obligatoria.
  for b in php php-fpm; do codesign --force --sign - "$PREFIX/$b" 2>/dev/null || true; done
fi

package "$PREFIX" "php-$VERSION" "$OUT/php-$VERSION-$OS-$ARCH.tar.gz"
