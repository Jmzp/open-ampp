# Open Ampp

Entorno local tipo XAMPP/LAMPP — **Apache + MariaDB + PHP** — con múltiples versiones de cada herramienta, UI de escritorio y soporte para Windows, macOS y Linux.

## Estructura

| Ruta | Contenido |
|---|---|
| `crates/ampp-core` | Motor en Rust (sin Tauri): rutas, estado, catálogo, instalación y servicios |
| `apps/desktop` | App Tauri 2 (`src-tauri/`) + UI React/TS (`src/`) |
| `templates/` | Plantillas de configuración (httpd.conf, php.ini, my.cnf, vhosts) |
| `catalog/` | Manifiesto de versiones descargables y su generador |
| `binaries/` | Scripts que compilan Apache/MariaDB portables y re-empaquetan PHP |

## Desarrollo

Requisitos: Rust estable, Node 22 y las [dependencias de Tauri](https://tauri.app/start/prerequisites/) de tu SO.

```sh
cargo test -p ampp-core          # tests del motor
cd apps/desktop && npm install
npm run tauri dev                # abre la app
```

## Hoja de ruta

1. ✅ Fundaciones: monorepo, `ampp-core` (rutas + `state.json`), scaffold Tauri/React, CI
2. ✅ Catálogo + instalador (descarga, sha256, extracción, pantalla de Versiones)
3. ✅ Servicios: Apache + PHP (FastCGI) + MariaDB, logs y health checks
4. ✅ Binarios portables propios para macOS/Linux (Apache, MariaDB, PHP CLI+FPM)
5. Proyectos y vhosts (`*.localhost`, PHP por proyecto, SSL)
6. Pulido: tray, editor de php.ini, Adminer, Composer, CLI
7. Distribución: instaladores firmados y auto-update

## Catálogo de versiones

`catalog/manifest.json` se genera desde las fuentes oficiales (php.net, static-php-cli, mariadb.org, Apache Lounge):

```sh
python3 catalog/generate.py
cargo test -p ampp-core -- --ignored   # descarga e inicia PHP real (requiere red)
```

## Binarios propios

Apache y MariaDB no publican builds portables para macOS/Linux, así que los compilamos
(`binaries/`) y se publican como releases `bin-<herramienta>-<versión>` desde el workflow
**Binarios** (`.github/workflows/binaries.yml`, ejecución manual). Son relocalizables:
funcionan desde cualquier carpeta y no dependen de Homebrew ni de paquetes del sistema
(en Linux se compilan sobre AlmaLinux 8, glibc ≥ 2.28).

```sh
WORK=/tmp/ampp-build binaries/build-apache.sh dist
WORK=/tmp/ampp-build binaries/build-mariadb.sh 11.4.13 dist   # requiere cmake ≥ 3.23 y bison ≥ 3
WORK=/tmp/ampp-build binaries/repack-php.sh 8.4.23 dist

# Prueba de punta a punta con los paquetes generados
OPEN_AMPP_APACHE_PKG=dist/apache-….tar.gz OPEN_AMPP_PHP_PKG=dist/php-….tar.gz \
OPEN_AMPP_MARIADB_PKG=dist/mariadb-….tar.gz cargo test -p ampp-core --test stack -- --ignored
```

Tras publicar nuevas releases, regenera el catálogo con `python3 catalog/generate.py`.
