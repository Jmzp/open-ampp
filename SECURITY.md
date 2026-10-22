# Security policy

## Reporting a vulnerability

Please **do not** report security problems in public issues. Report them privately through GitHub instead: [**Report a vulnerability**](https://github.com/Jmzp/open-ampp/security/advisories/new). Include what you found, how to reproduce it and which version and platform you used.

You will get an answer as soon as possible. Once a fix is ready, it is released and the advisory is published, crediting you if you wish.

## What is in scope

- The Open AMPP app, the `ampp` CLI and `ampp-core`.
- The configuration Open AMPP generates (`httpd.conf`, `php.ini`, `my.cnf`) and how it launches services.
- The binaries Open AMPP builds and publishes (`bin-*` releases) and the download catalog.

Vulnerabilities in **PHP, Apache, MariaDB, phpMyAdmin or Composer themselves** belong to those projects; please report them upstream. What Open AMPP does about them is ship the fixed versions quickly: PHP builds are checked daily, the catalog is updated without a new app release, and `ampp upgrade` (or the app's update button) moves you to the patched version.

## Supported versions

Only the latest release receives fixes; the app updates itself.

## By design

Open AMPP is meant for **local development**, not for production servers. Services listen on `127.0.0.1` only and MariaDB's `root` user has no password. Exposing those ports to a network is outside the supported setup.
