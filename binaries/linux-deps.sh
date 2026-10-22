#!/usr/bin/env bash
# Installs the build dependencies on AlmaLinux 8 (glibc 2.28), the Linux build environment.
# Used by CI and for local testing: docker run … almalinux:8 bash -c '/src/linux-deps.sh && …'
set -euo pipefail
dnf install -y -q epel-release
# gcc-toolset-12: a modern compiler whose output still runs on EL8's glibc/libstdc++.
dnf install -y -q gcc-toolset-12 make perl perl-IPC-Cmd patchelf curl file python3 zlib-devel \
  cmake bison ncurses-devel git pkgconf-pkg-config tar gzip diffutils findutils which
