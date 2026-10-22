# Versiones y SHA-256 de las fuentes, tomados de las publicaciones oficiales.
# shellcheck disable=SC2034

HTTPD_VERSION=2.4.69
HTTPD_URL="https://archive.apache.org/dist/httpd/httpd-${HTTPD_VERSION}.tar.gz"
HTTPD_SHA256=c551b9861e26f146f57e1b1765e69c89a60b3db0ec252ccab0d36283487016bb

APR_VERSION=1.7.6
APR_URL="https://archive.apache.org/dist/apr/apr-${APR_VERSION}.tar.gz"
APR_SHA256=6a10e7f7430510600af25fabf466e1df61aaae910bf1dc5d10c44a4433ccc81d

APRUTIL_VERSION=1.6.5
APRUTIL_URL="https://archive.apache.org/dist/apr/apr-util-${APRUTIL_VERSION}.tar.gz"
APRUTIL_SHA256=f43a1c8c79eef497a022ec6c99dddbdf57e42001da6ccbfae259631ed5aa2805

PCRE2_VERSION=10.49
PCRE2_URL="https://github.com/PCRE2Project/pcre2/releases/download/pcre2-${PCRE2_VERSION}/pcre2-${PCRE2_VERSION}.tar.gz"
PCRE2_SHA256=929f0b20e62879252a15886b06c89f1edef61a363cbd5826fb041080a5e557ae

EXPAT_VERSION=2.9.0
EXPAT_URL="https://github.com/libexpat/libexpat/releases/download/R_${EXPAT_VERSION//./_}/expat-${EXPAT_VERSION}.tar.gz"
EXPAT_SHA256=16afbb9cefead2aa278105cf27d9f597bde7fbf3dbb85015857ca7ca6a4e89ba

OPENSSL_VERSION=3.5.9
OPENSSL_URL="https://github.com/openssl/openssl/releases/download/openssl-${OPENSSL_VERSION}/openssl-${OPENSSL_VERSION}.tar.gz"
OPENSSL_SHA256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a

# MariaDB: una entrada por versión LTS soportada.
mariadb_sha256() {
  case "$1" in
    10.11.19) echo b8e543ee69d380fb1cfd563226f49e0fe96e4d67e7b7a9045ee514a168ed2066 ;;
    11.4.13) echo 1bb254b106d0a7ca871cfa18fa6e18d4b80a7430f9ec9d1571ec4271a13def96 ;;
    11.8.9) echo 2e015b7e91004142eda8b3a20d3ceb875cf2ae55e72603e7222cd497d4e56fc6 ;;
    *) return 1 ;;
  esac
}
mariadb_url() { echo "https://archive.mariadb.org/mariadb-$1/source/mariadb-$1.tar.gz"; }
