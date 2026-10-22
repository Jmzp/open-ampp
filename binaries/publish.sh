#!/usr/bin/env bash
# Sube cada paquete a su release `bin-<herramienta>-<versión>` (la crea si no existe).
# Uso: GH_TOKEN=… ./publish.sh <directorio-con-tar.gz>
set -euo pipefail
DIR=${1:?uso: publish.sh <directorio>}
shopt -s nullglob

for file in "$DIR"/*.tar.gz; do
  name=$(basename "$file")
  [[ $name =~ ^(apache|mariadb|php)-([0-9.]+)-(macos|linux)-(x64|arm64)\.tar\.gz$ ]] || { echo "ignorando $name"; continue; }
  tag="bin-${BASH_REMATCH[1]}-${BASH_REMATCH[2]}"
  if ! gh release view "$tag" >/dev/null 2>&1; then
    gh release create "$tag" --title "${BASH_REMATCH[1]} ${BASH_REMATCH[2]} (binarios)" \
      --notes "Binarios portables de ${BASH_REMATCH[1]} ${BASH_REMATCH[2]} usados por Open Ampp."
  fi
  echo "subiendo $name → $tag"
  gh release upload "$tag" "$file" --clobber
done
