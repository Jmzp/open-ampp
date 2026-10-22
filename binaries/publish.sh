#!/usr/bin/env bash
# Uploads each package to its `bin-<tool>-<version>` release (created if missing).
# Usage: GH_TOKEN=… ./publish.sh <dir-with-tar.gz>
set -euo pipefail
DIR=${1:?usage: publish.sh <dir>}
shopt -s nullglob

for file in "$DIR"/*.tar.gz; do
  name=$(basename "$file")
  [[ $name =~ ^(apache|mariadb|php)-([0-9.]+)-(macos|linux)-(x64|arm64)\.tar\.gz$ ]] || { echo "skipping $name"; continue; }
  tag="bin-${BASH_REMATCH[1]}-${BASH_REMATCH[2]}"
  if ! gh release view "$tag" >/dev/null 2>&1; then
    gh release create "$tag" --title "${BASH_REMATCH[1]} ${BASH_REMATCH[2]} (binaries)" --latest=false \
      --notes "Portable ${BASH_REMATCH[1]} ${BASH_REMATCH[2]} binaries used by Open AMPP."
  fi
  echo "uploading $name → $tag"
  gh release upload "$tag" "$file" --clobber
done
