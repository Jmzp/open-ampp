# Funciones comunes de los scripts de build. Se carga con `source`.
set -euo pipefail

log() { printf '\033[1;34m==>\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux) OS=linux ;;
  *) die "sistema no soportado: $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x64 ;;
  arm64 | aarch64) ARCH=arm64 ;;
  *) die "arquitectura no soportada: $(uname -m)" ;;
esac

if [ "$OS" = macos ]; then
  JOBS=$(sysctl -n hw.ncpu)
  # Compatibilidad con macOS 11 en adelante.
  export MACOSX_DEPLOYMENT_TARGET=11.0
else
  JOBS=$(nproc)
  if [ -f /opt/rh/gcc-toolset-12/enable ]; then
    set +u
    # shellcheck disable=SC1091
    source /opt/rh/gcc-toolset-12/enable
    set -u
  fi
fi

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# fetch <url> <sha256> <directorio-destino>: descarga (con caché), verifica y extrae. Imprime la carpeta extraída.
fetch() {
  local url=$1 expected=$2 dest=$3 file
  file="$WORK/downloads/$(basename "$url")"
  mkdir -p "$WORK/downloads" "$dest"
  if [ ! -f "$file" ] || [ "$(sha256 "$file")" != "$expected" ]; then
    log "descargando $(basename "$url")"
    curl -fsSL --retry 3 -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  local actual
  actual=$(sha256 "$file")
  [ "$actual" = "$expected" ] || die "checksum inválido para $file: $actual != $expected"
  local top
  top=$(tar -tzf "$file" | head -1 | cut -d/ -f1)
  rm -rf "${dest:?}/$top"
  tar -xzf "$file" -C "$dest"
  echo "$dest/$top"
}

# build_openssl <prefix>: OpenSSL estática (requiere sources.sh cargado).
# `no-dso` evita depender de dlopen/libdl, que en glibc < 2.34 es una librería aparte.
build_openssl() {
  local prefix=$1 dir
  if [ -f "$prefix/lib/libssl.a" ]; then return; fi
  log "OpenSSL $OPENSSL_VERSION (estática)"
  dir=$(fetch "$OPENSSL_URL" "$OPENSSL_SHA256" "$WORK/src")
  (cd "$dir" && make distclean >/dev/null 2>&1 || true)
  (cd "$dir" && ./Configure --prefix="$prefix" --libdir=lib no-shared no-dso no-tests no-docs >/dev/null \
    && make -j"$JOBS" >/dev/null && make install_sw >/dev/null)
}

is_macho() { file -b "$1" | grep -q 'Mach-O'; }
is_elf() { file -b "$1" | grep -q 'ELF'; }

# Ejecutables y librerías dentro de un prefix.
binaries_in() {
  find "$1" -type f \( -perm -u+x -o -name '*.so' -o -name '*.dylib' -o -name '*.so.*' \) | while read -r f; do
    if is_macho "$f" || is_elf "$f"; then echo "$f"; fi
  done
}

# relocate <prefix>: hace que el árbol funcione desde cualquier ruta.
#  - macOS: dependencias dentro del prefix → @rpath, rpath relativo a cada binario, firma ad-hoc.
#  - Linux: copia las librerías no básicas a lib/ y fija RUNPATH=$ORIGIN/…/lib.
relocate() {
  local prefix=$1
  mkdir -p "$prefix/lib"
  if [ "$OS" = macos ]; then relocate_macos "$prefix"; else relocate_linux "$prefix"; fi
}

relocate_macos() {
  local prefix=$1 f dep rel
  binaries_in "$prefix" | while read -r f; do
    chmod u+w "$f"
    case "$f" in *.dylib) install_name_tool -id "@rpath/$(basename "$f")" "$f" 2>/dev/null ;; esac
    otool -L "$f" | tail -n +2 | awk '{print $1}' | while read -r dep; do
      case "$dep" in
        "$prefix"/* | "$WORK"/*) install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$f" 2>/dev/null ;;
        /usr/lib/* | /System/* | @*) ;;
        *) die "$f depende de $dep, que no existe en un macOS limpio" ;;
      esac
    done
    rel=$(python3 -c 'import os,sys; print(os.path.relpath(sys.argv[1], os.path.dirname(sys.argv[2])))' "$prefix/lib" "$f")
    install_name_tool -add_rpath "@loader_path/$rel" "$f" 2>/dev/null || true
    codesign --force --sign - "$f" 2>/dev/null
  done
}

# Librerías que se asumen presentes en cualquier Linux con glibc.
LINUX_BASE_LIBS='^(linux-vdso|linux-gate|ld-linux|libc\.so|libm\.so|libpthread\.so|libdl\.so|librt\.so|libresolv\.so|libutil\.so)'

relocate_linux() {
  local prefix=$1 f lib name path rel
  command -v patchelf >/dev/null || die "falta patchelf"
  # Dos pasadas: las librerías copiadas también pueden traer dependencias.
  for _ in 1 2; do
    binaries_in "$prefix" | while read -r f; do
      ldd "$f" 2>/dev/null | awk '/=>/ {print $1, $3}' | while read -r name path; do
        [[ $name =~ $LINUX_BASE_LIBS ]] && continue
        [ -f "$path" ] || die "$f: no se encontró $name"
        [ -f "$prefix/lib/$name" ] || { log "incluyendo $name"; cp -L "$path" "$prefix/lib/$name"; chmod u+w "$prefix/lib/$name"; }
      done
    done
  done
  binaries_in "$prefix" | while read -r f; do
    rel=$(python3 -c 'import os,sys; print(os.path.relpath(sys.argv[1], os.path.dirname(sys.argv[2])))' "$prefix/lib" "$f")
    patchelf --set-rpath "\$ORIGIN/$rel" "$f"
  done
}

# verify_relocatable <prefix>: falla si algún binario aún apunta al directorio de build.
verify_relocatable() {
  local prefix=$1 f bad=0
  while read -r f; do
    if [ "$OS" = macos ]; then
      if otool -L "$f" | tail -n +2 | grep -qE "$WORK|$prefix"; then log "referencia absoluta en $f"; bad=1; fi
    else
      if ldd "$f" 2>/dev/null | grep -q 'not found'; then log "dependencia faltante en $f"; ldd "$f" | grep 'not found' >&2; bad=1; fi
    fi
  done < <(binaries_in "$prefix")
  [ $bad = 0 ] || die "el árbol no es relocalizable"
}

# package <prefix> <nombre-raíz> <archivo.tar.gz>
package() {
  local prefix=$1 root=$2 out=$3 stage="$WORK/stage"
  rm -rf "$stage" && mkdir -p "$stage" "$(dirname "$out")"
  cp -a "$prefix" "$stage/$root"
  # Sin archivos AppleDouble (._*) con atributos extendidos de macOS.
  COPYFILE_DISABLE=1 tar -czf "$out" -C "$stage" "$root"
  rm -rf "$stage"
  log "paquete: $out ($(du -h "$out" | cut -f1), sha256 $(sha256 "$out"))"
}
