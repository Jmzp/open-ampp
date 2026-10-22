# OPEN_AMPP

Entorno local tipo XAMPP/LAMPP — **Apache + MariaDB + PHP** — con múltiples versiones de cada herramienta, UI de escritorio y soporte para Windows, macOS y Linux.

## Estructura

| Ruta | Contenido |
|---|---|
| `crates/ampp-core` | Motor en Rust (sin Tauri): rutas, estado, catálogo, instalación y servicios |
| `apps/desktop` | App Tauri 2 (`src-tauri/`) + UI React/TS (`src/`) |
| `templates/` | Plantillas de configuración (httpd.conf, php.ini, my.cnf, vhosts) |
| `catalog/` | Manifiesto de versiones descargables |

## Desarrollo

Requisitos: Rust estable, Node 22 y las [dependencias de Tauri](https://tauri.app/start/prerequisites/) de tu SO.

```sh
cargo test -p ampp-core          # tests del motor
cd apps/desktop && npm install
npm run tauri dev                # abre la app
```

## Hoja de ruta

1. ✅ Fundaciones: monorepo, `ampp-core` (rutas + `state.json`), scaffold Tauri/React, CI
2. Catálogo + instalador (descarga, sha256, extracción)
3. Servicios: Apache + PHP (FastCGI) + MariaDB, logs y health checks
4. Pipelines de binarios portables para macOS/Linux
5. Proyectos y vhosts (`*.localhost`, PHP por proyecto, SSL)
6. Pulido: tray, editor de php.ini, Adminer, Composer, CLI
7. Distribución: instaladores firmados y auto-update
