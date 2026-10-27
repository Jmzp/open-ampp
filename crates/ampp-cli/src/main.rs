//! `ampp`: Open AMPP from the terminal. Shares data and state with the desktop app.

use std::process::ExitCode;

use ampp_core::catalog::compare_versions;
use ampp_core::state::Project;
use ampp_core::{AppPaths, Extra, Manifest, Platform, State, Supervisor, Tool, installer, projects, tls, tools, tr};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "ampp", version, about = "Open AMPP: Apache, MariaDB and PHP for local development")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the status of the services.
    Status,
    /// Start the services in the foreground; Ctrl+C stops them.
    Up,
    /// Stop one service (or all of them), even if the app started it.
    Stop { service: Option<ServiceArg> },
    /// List installed and available versions.
    List,
    /// Move installed versions to their newest patch release (security fixes).
    Upgrade { tool: Option<ToolArg> },
    /// Install a version (the latest by default) or a tool (phpmyadmin, composer).
    Install { target: Target, version: Option<String> },
    /// Uninstall a version or a tool.
    Uninstall { target: Target, version: Option<String> },
    /// Choose the active version of a tool.
    Use { tool: ToolArg, version: String },
    /// Sites served on their own domain (blog.localhost) from any folder.
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Trust the local CA that signs the HTTPS projects (the system asks for confirmation).
    Trust,
    /// Set the MariaDB root password of the active series (omit it to remove it). MariaDB must run.
    RootPassword { password: Option<String> },
    /// Run the active PHP with the Open AMPP php.ini.
    #[command(disable_help_flag = true)]
    Php {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run Composer with the active PHP.
    #[command(disable_help_flag = true)]
    Composer {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Open the MariaDB client connected as root.
    #[command(disable_help_flag = true)]
    Mariadb {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Regenerate the shims (php, composer, mariadb…) and optionally add them to the PATH.
    Shims {
        #[arg(long)]
        add_to_path: bool,
    },
    /// Print where Open AMPP keeps its data.
    Paths,
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// List the projects.
    Ls,
    /// Serve a folder on its own domain (NAME.localhost by default).
    Add {
        name: String,
        /// Folder served as DocumentRoot (for Laravel, its public/ folder).
        folder: String,
        #[arg(long)]
        domain: Option<String>,
        /// PHP series, e.g. 8.2 (the active version by default).
        #[arg(long)]
        php: Option<String>,
        /// Also serve it over HTTPS with the local CA (.localhost domains only).
        #[arg(long)]
        https: bool,
    },
    /// Stop serving a project (its folder is not deleted).
    Rm { name: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum ToolArg {
    Apache,
    Php,
    Mariadb,
}

impl From<ToolArg> for Tool {
    fn from(t: ToolArg) -> Self {
        match t {
            ToolArg::Apache => Tool::Apache,
            ToolArg::Php => Tool::Php,
            ToolArg::Mariadb => Tool::Mariadb,
        }
    }
}

/// Something `ampp stop` can stop: a service or Mailpit.
#[derive(Clone, Copy, ValueEnum)]
enum ServiceArg {
    Apache,
    Php,
    Mariadb,
    Mailpit,
}

#[derive(Clone, Copy, ValueEnum)]
enum Target {
    Apache,
    Php,
    Mariadb,
    Phpmyadmin,
    Composer,
    Mailpit,
}

enum Resolved {
    Tool(Tool),
    Extra(Extra),
}

impl Target {
    fn resolve(self) -> Resolved {
        match self {
            Target::Apache => Resolved::Tool(Tool::Apache),
            Target::Php => Resolved::Tool(Tool::Php),
            Target::Mariadb => Resolved::Tool(Tool::Mariadb),
            Target::Phpmyadmin => Resolved::Extra(Extra::Phpmyadmin),
            Target::Composer => Resolved::Extra(Extra::Composer),
            Target::Mailpit => Resolved::Extra(Extra::Mailpit),
        }
    }
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Ctx {
    paths: AppPaths,
    state: State,
}

impl Ctx {
    fn load() -> Result<Self> {
        let paths = AppPaths::from_system()?;
        paths.ensure()?;
        let state = State::load(&paths.state_file())?;
        Ok(Self { paths, state })
    }

    /// Saves the state and keeps the shims up to date.
    fn save(&self) -> Result<()> {
        self.state.save(&self.paths.state_file())?;
        let cli = std::env::current_exe().ok();
        tools::write_shims(&self.paths, &self.state, cli.as_deref())?;
        Ok(())
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let mut ctx = Ctx::load()?;
    match cli.command {
        Cmd::Status => status(&ctx, &Supervisor::new(ctx.paths.clone())).await,
        Cmd::Up => return up(&ctx).await,
        Cmd::Stop { service } => {
            let sup = Supervisor::new(ctx.paths.clone());
            match service {
                Some(ServiceArg::Apache) => sup.stop(Tool::Apache).await?,
                Some(ServiceArg::Php) => sup.stop(Tool::Php).await?,
                Some(ServiceArg::Mariadb) => sup.stop(Tool::Mariadb).await?,
                Some(ServiceArg::Mailpit) => sup.stop_mail().await,
                None => sup.stop_all().await,
            }
            status(&ctx, &sup).await;
        }
        Cmd::List => list(&ctx).await,
        Cmd::Upgrade { tool } => upgrade(&mut ctx, tool.map(Tool::from)).await?,
        Cmd::Install { target, version } => install(&mut ctx, target, version).await?,
        Cmd::Uninstall { target, version } => uninstall(&mut ctx, target, version)?,
        Cmd::Use { tool, version } => {
            ctx.state.set_active(tool.into(), &version)?;
            ctx.save()?;
            let tool = Tool::from(tool);
            println!("{}", tr!("{tool} {version} activo. Reinicia el servicio si está en marcha.", "{tool} {version} is now active. Restart the service if it is running."));
        }
        Cmd::Project(cmd) => project(&mut ctx, cmd)?,
        Cmd::Trust => trust(&ctx)?,
        Cmd::RootPassword { password } => root_password(&mut ctx, password.unwrap_or_default()).await?,
        Cmd::Php { args } => return passthrough(&ctx, "php", args),
        Cmd::Composer { args } => return passthrough(&ctx, "composer", args),
        Cmd::Mariadb { args } => return passthrough(&ctx, "mariadb", args),
        Cmd::Shims { add_to_path } => {
            let cli = std::env::current_exe().ok();
            let names = tools::write_shims(&ctx.paths, &ctx.state, cli.as_deref())?;
            let dir = tools::shims_dir(&ctx.paths);
            let (dir, names) = (dir.display(), names.join(", "));
            println!("{}", tr!("Atajos en {dir}: {names}", "Shims in {dir}: {names}"));
            if add_to_path {
                for place in tools::add_shims_to_path(&ctx.paths)? {
                    println!("{}", tr!("PATH actualizado en {place}", "PATH updated in {place}"));
                }
                println!("{}", tr!("Abre una terminal nueva para usarlos.", "Open a new terminal to use them."));
            } else if !tools::shims_on_path(&ctx.paths) {
                println!("{}", tr!("Para usarlos en cualquier terminal: ampp shims --add-to-path", "To use them from any terminal: ampp shims --add-to-path"));
            }
        }
        Cmd::Paths => println!("{}", ctx.paths.root().display()),
    }
    Ok(ExitCode::SUCCESS)
}

async fn status(ctx: &Ctx, sup: &Supervisor) {
    for s in sup.status().await {
        let name = s.tool.label();
        if s.running {
            let port = s.port.map(|p| tr!(" en el puerto {p}", " on port {p}")).unwrap_or_default();
            let extra = match s.extra_php.iter().map(|e| e.version.as_str()).collect::<Vec<_>>().join(", ") {
                v if v.is_empty() => String::new(),
                v => tr!(" (+{v} para proyectos)", " (+{v} for projects)"),
            };
            println!("● {name:<8} {}{port}, PID {}{extra}", s.version.unwrap_or_default(), s.pid.unwrap_or(0));
        } else {
            let active = ctx.state.active.get(&s.tool).cloned().unwrap_or_else(|| tr!("sin instalar", "not installed"));
            println!("○ {name:<8} {}", tr!("detenido ({active})", "stopped ({active})"));
        }
    }
    let mail = sup.mail_status().await;
    if mail.running {
        let port = mail.web_port.unwrap_or(ctx.state.ports.mail_web);
        println!("● {:<8} {} http://localhost:{port}, PID {}", "Mailpit", mail.version.unwrap_or_default(), mail.pid.unwrap_or(0));
    } else if mail.installed {
        println!("○ {:<8} {}", "Mailpit", tr!("detenido", "stopped"));
    }
}

async fn up(ctx: &Ctx) -> Result<ExitCode> {
    let sup = Supervisor::new(ctx.paths.clone());
    sup.cleanup_orphans().await;
    let failures = sup.start_all(&ctx.state).await;
    for e in &failures {
        eprintln!("✗ {e}");
    }
    status(ctx, &sup).await;
    let running = sup.status().await.iter().any(|s| s.running && !s.external);
    if !running {
        return Ok(ExitCode::FAILURE);
    }
    let port = ctx.state.ports.http;
    println!("\n{}", tr!("http://localhost:{port}  ·  Ctrl+C para detener", "http://localhost:{port}  ·  Ctrl+C to stop"));
    let signal = shutdown_signal();
    tokio::pin!(signal);
    loop {
        tokio::select! {
            _ = &mut signal => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                // If something else stopped them (the app, `ampp stop`), there is nothing left to wait for.
                if !sup.status().await.iter().any(|s| s.running && !s.external) {
                    println!("{}", tr!("Los servicios se detuvieron.", "The services were stopped."));
                    return Ok(ExitCode::SUCCESS);
                }
            }
        }
    }
    println!("\n{}", tr!("Deteniendo…", "Stopping…"));
    sup.shutdown().await;
    Ok(if failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Ctrl+C or, on Unix, SIGTERM (e.g. when the terminal closes or a process manager stops us).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("could not listen for SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

async fn list(ctx: &Ctx) {
    let manifest = Manifest::latest().await;
    let platform = Platform::current();
    let available = Platform::current().map(|p| manifest.for_platform(p)).unwrap_or_default();
    for tool in Tool::ALL {
        println!("{tool}");
        let installed = ctx.state.installed.get(&tool).cloned().unwrap_or_default();
        for v in &installed {
            let active = ctx.state.active.get(&tool) == Some(v);
            let mut note = if active { tr!("  (activa)", "  (active)") } else { String::new() };
            if let Some(new) = platform.and_then(|p| manifest.patch_update(tool, v, p)) {
                let new = &new.version;
                note += &tr!("  → {new} disponible (ampp upgrade)", "  → {new} available (ampp upgrade)");
            }
            println!("  {} {v}{note}", if active { "●" } else { "○" });
        }
        for p in available.iter().filter(|p| p.tool == tool && !installed.contains(&p.version)) {
            println!("    {}  {}", p.version, tr!("disponible", "available"));
        }
    }
    println!("{}", tr!("Herramientas", "Tools"));
    for extra in Extra::ALL {
        match (ctx.state.extras.get(&extra), manifest.extra(extra)) {
            (Some(v), _) => println!("  ● {} {v}", extra.label()),
            (None, Some(e)) => println!("    {} {}  {}", extra.label(), e.version, tr!("disponible", "available")),
            (None, None) => {}
        }
    }
}

async fn install(ctx: &mut Ctx, target: Target, version: Option<String>) -> Result<()> {
    let manifest = Manifest::latest().await;
    match target.resolve() {
        Resolved::Tool(tool) => {
            let platform = Platform::current().ok_or_else(|| tr!("plataforma no soportada", "unsupported platform"))?;
            let pkg = match version {
                Some(v) => manifest.find(tool, &v, platform)?.clone(),
                None => manifest
                    .for_platform(platform)
                    .into_iter()
                    .filter(|p| p.tool == tool)
                    .max_by(|a, b| compare_versions(&a.version, &b.version))
                    .ok_or_else(|| tr!("no hay versiones de {tool} para esta plataforma", "there are no {tool} versions for this platform"))?
                    .clone(),
            };
            let last = std::sync::Mutex::new(0u64);
            installer::install(&ctx.paths, &pkg, |p| {
                // One line every 10 MB downloaded.
                let mut last = last.lock().unwrap();
                if p.downloaded >= *last + 10 * 1024 * 1024 {
                    *last = p.downloaded;
                    eprintln!("  {} MB…", p.downloaded / (1024 * 1024));
                }
            })
            .await?;
            ctx.state.mark_installed(tool, &pkg.version);
            ctx.save()?;
            let version = &pkg.version;
            println!("{}", tr!("{tool} {version} instalado.", "{tool} {version} installed."));
        }
        Resolved::Extra(extra) => {
            let file = manifest.extra(extra).ok_or_else(|| ampp_core::Error::ExtraNotInCatalog(extra.as_str().into()))?;
            tools::install(&ctx.paths, file).await?;
            ctx.state.extras.insert(extra, file.version.clone());
            ctx.save()?;
            let (name, version) = (extra.label(), &file.version);
            println!("{}", tr!("{name} {version} instalado.", "{name} {version} installed."));
        }
    }
    Ok(())
}

/// Installs the newest patch of every installed version, switches the active one, and removes
/// the old version. Running services keep the old binaries until restarted, so those stay.
async fn upgrade(ctx: &mut Ctx, only: Option<Tool>) -> Result<()> {
    let manifest = Manifest::latest().await;
    let platform = Platform::current().ok_or_else(|| tr!("plataforma no soportada", "unsupported platform"))?;
    let running = Supervisor::new(ctx.paths.clone()).status().await;
    let mut upgraded = 0;
    for tool in Tool::ALL.into_iter().filter(|t| only.is_none_or(|o| o == *t)) {
        for old in ctx.state.installed.get(&tool).cloned().unwrap_or_default() {
            let Some(pkg) = manifest.patch_update(tool, &old, platform).cloned() else { continue };
            let new = &pkg.version;
            println!("{}", tr!("{tool} {old} → {new}…", "{tool} {old} → {new}…"));
            installer::install(&ctx.paths, &pkg, |_| {}).await?;
            ctx.state.mark_installed(tool, new);
            if ctx.state.active.get(&tool) == Some(&old) {
                ctx.state.set_active(tool, new)?;
            }
            let in_use = running.iter().any(|s| s.tool == tool && s.running && s.version.as_deref() == Some(&old));
            if in_use {
                println!("{}", tr!("  {old} sigue en uso: reinicia {tool} para usar {new}.", "  {old} is still running: restart {tool} to use {new}."));
            } else {
                installer::uninstall(&ctx.paths, tool, &old)?;
                ctx.state.mark_uninstalled(tool, &old);
            }
            upgraded += 1;
        }
    }
    ctx.save()?;
    if upgraded == 0 {
        println!("{}", tr!("Todo está al día.", "Everything is up to date."));
    }
    Ok(())
}

fn uninstall(ctx: &mut Ctx, target: Target, version: Option<String>) -> Result<()> {
    match target.resolve() {
        Resolved::Tool(tool) => {
            let version = version.ok_or_else(|| tr!("indica la versión a desinstalar", "say which version to uninstall"))?;
            installer::uninstall(&ctx.paths, tool, &version)?;
            ctx.state.mark_uninstalled(tool, &version);
            println!("{}", tr!("{tool} {version} desinstalado.", "{tool} {version} uninstalled."));
        }
        Resolved::Extra(extra) => {
            tools::uninstall(&ctx.paths, extra)?;
            ctx.state.extras.remove(&extra);
            let name = extra.label();
            println!("{}", tr!("{name} desinstalado.", "{name} uninstalled."));
        }
    }
    ctx.save()
}

fn project(ctx: &mut Ctx, cmd: ProjectCmd) -> Result<()> {
    match cmd {
        ProjectCmd::Ls => {
            if ctx.state.projects.is_empty() {
                println!("{}", tr!("No hay proyectos. Crea uno con: ampp project add blog ~/sites/blog", "No projects. Create one with: ampp project add blog ~/sites/blog"));
            }
            for p in &ctx.state.projects {
                let php = match projects::php_for(&ctx.paths, &ctx.state, p.php.as_deref()) {
                    Ok(inst) => format!("PHP {}", inst.version),
                    Err(_) => tr!("PHP {} no instalado", "PHP {} not installed", p.php.as_deref().unwrap_or("?")),
                };
                println!("{:<16} {:<36} {php:<14} {}", p.name, projects::url(&ctx.state, p), p.docroot.display());
            }
            return Ok(());
        }
        ProjectCmd::Add { name, folder, domain, php, https } => {
            let docroot = std::fs::canonicalize(&folder).unwrap_or_else(|_| folder.into());
            let domain = domain.unwrap_or_else(|| projects::suggested_domain(&name));
            let project = Project { name: name.clone(), domain, docroot, php, https };
            projects::save(&ctx.paths, &mut ctx.state, None, project)?;
            if let Some(p) = ctx.state.projects.iter().find(|p| p.name == name.trim()) {
                println!("{} → {}", p.name, projects::url(&ctx.state, p));
            }
        }
        ProjectCmd::Rm { name } => {
            projects::remove(&mut ctx.state, &name)?;
        }
    }
    ctx.save()?;
    println!("{}", tr!("Listo. Reinicia Apache y PHP para aplicarlo (ampp stop y ampp up).", "Done. Restart Apache and PHP to apply it (ampp stop, then ampp up)."));
    Ok(())
}

async fn root_password(ctx: &mut Ctx, password: String) -> Result<()> {
    ampp_core::db::set_root_password(&ctx.paths, &ctx.state, &password).await?;
    let id = ctx.state.active.get(&Tool::Mariadb).cloned().unwrap_or_default();
    let series = ampp_core::install::locate(&ctx.paths, Tool::Mariadb, &id)?.minor_version();
    match password.is_empty() {
        true => ctx.state.mariadb_root_passwords.remove(&series),
        false => ctx.state.mariadb_root_passwords.insert(series.clone(), password.clone()),
    };
    ampp_core::db::client_options(&ctx.paths, &ctx.state)?;
    ctx.save()?;
    if password.is_empty() {
        println!("{}", tr!("root de MariaDB {series} ya no tiene contraseña.", "MariaDB {series} root no longer has a password."));
    } else {
        println!("{}", tr!("Contraseña de root de MariaDB {series} cambiada. Reinicia Apache para phpMyAdmin.", "MariaDB {series} root password changed. Restart Apache for phpMyAdmin."));
    }
    Ok(())
}

fn trust(ctx: &Ctx) -> Result<()> {
    let ca = tls::ca_cert_path(&ctx.paths);
    if tls::is_trusted(&ctx.paths) == Some(true) {
        println!("{}", tr!("La CA local ya es de confianza ({}).", "The local CA is already trusted ({}).", ca.display()));
        return Ok(());
    }
    tls::trust(&ctx.paths)?;
    println!("{}", tr!("Listo: la CA local es de confianza. Reinicia el navegador.", "Done: the local CA is trusted. Restart your browser."));
    Ok(())
}

/// Runs a tool command (php, composer, mariadb) attached to the current terminal.
fn passthrough(ctx: &Ctx, name: &str, args: Vec<String>) -> Result<ExitCode> {
    let cmd = tools::commands(&ctx.paths, &ctx.state, None)?
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| tr!("{name} no está disponible: instálalo con `ampp install {name}`", "{name} is not available: install it with `ampp install {name}`"))?;
    let mut proc = std::process::Command::new(&cmd.program);
    proc.args(&cmd.args).args(&args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(proc.exec().into())
    }
    #[cfg(not(unix))]
    {
        let status = proc.status()?;
        Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
    }
}
