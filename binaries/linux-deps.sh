#!/usr/bin/env bash
# Instala las dependencias de build en AlmaLinux 8 (glibc 2.28), el entorno de los builds de Linux.
# Se usa en el CI y para probar en local: docker run … almalinux:8 bash -c '/src/linux-deps.sh && …'
set -euo pipefail
dnf install -y -q epel-release
# gcc-toolset-12: compilador moderno que sigue generando binarios compatibles con el glibc/libstdc++ de EL8.
dnf install -y -q gcc-toolset-12 make perl perl-IPC-Cmd patchelf curl file python3 zlib-devel \
  cmake bison ncurses-devel git pkgconf-pkg-config tar gzip diffutils findutils which
