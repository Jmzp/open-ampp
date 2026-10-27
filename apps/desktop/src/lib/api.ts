import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Tool = "apache" | "php" | "mariadb";

export const TOOL_LABELS: Record<Tool, string> = {
  apache: "Apache",
  php: "PHP",
  mariadb: "MariaDB",
};

export interface Ports {
  http: number;
  https: number;
  mariadb: number;
  php_fcgi_base: number;
  mail_smtp: number;
  mail_web: number;
}

export interface Project {
  name: string;
  domain: string;
  docroot: string;
  /** PHP series ("8.2"); `null` uses the active version. */
  php: string | null;
  https: boolean;
}

export interface ProjectInfo extends Project {
  url: string;
  /** PHP version that serves it; `null` if its series is no longer installed (the active one serves it). */
  php_version: string | null;
  folder_exists: boolean;
}

export interface ProjectsInfo {
  projects: ProjectInfo[];
  /** Installed PHP series, newest first. */
  php_series: string[];
  active_series: string | null;
}

export type Extra = "phpmyadmin" | "composer" | "mailpit";

export interface PhpSettings {
  directives: Record<string, string>;
  /** Enabled extensions (Windows only). `null` means the defaults (XAMPP's). */
  extensions: string[] | null;
  custom: string;
}

export interface AppState {
  installed: Partial<Record<Tool, string[]>>;
  active: Partial<Record<Tool, string>>;
  ports: Ports;
  projects: Project[];
  php: PhpSettings;
  extras: Partial<Record<Extra, string>>;
}

export interface PhpConfig {
  settings: PhpSettings;
  defaults: Record<string, string>;
  version: string | null;
  /** Every installed PHP version: the settings apply to all of them. */
  installed: string[];
  loaded: string[];
  optional: { name: string; enabled: boolean }[];
  ini_path: string | null;
}

export interface TlsInfo {
  /** Some project uses HTTPS. */
  used: boolean;
  https_port: number;
  ca_exists: boolean;
  ca_path: string;
  /** `null` when it cannot be told (Linux). */
  trusted: boolean | null;
  /** Linux only: commands to trust the CA (they need sudo). */
  linux_commands: string | null;
}

export interface ToolsInfo {
  extras: { name: Extra; installed: string | null; available: string | null }[];
  shims_dir: string;
  on_path: boolean;
  commands: string[];
}

export type Location = "www" | "data" | "logs" | "php_ini" | "certs";

export interface MariadbInfo {
  connection: { host: string; port: number; user: string; password: string; socket: string | null };
  /** `null` while MariaDB is not running. */
  databases: { name: string; tables: number; size: number; system: boolean }[] | null;
}

export interface CatalogEntry {
  tool: Tool;
  /** Id: "8.3.12" or "homebrew". */
  version: string;
  /** Actual version when it differs from the id (Homebrew). */
  detail: string | null;
  size: number | null;
  installed: boolean;
  active: boolean;
  system: boolean;
  /** Newer patch release of this installed version (e.g. a security fix). */
  update: string | null;
}

export interface ServiceStatus {
  tool: Tool;
  running: boolean;
  pid: number | null;
  version: string | null;
  port: number | null;
  error: string | null;
  /** Launched from a terminal (`ampp up`). */
  external: boolean;
  /** PHP only: other versions running for projects, newest first. */
  extra_php: { version: string; port: number; pid: number | null }[];
}

export interface MailStatus {
  installed: boolean;
  running: boolean;
  pid: number | null;
  version: string | null;
  web_port: number | null;
  error: string | null;
  external: boolean;
}

export const HOMEBREW = "homebrew";

/** Display text for a version ("homebrew" → "Homebrew 2.4.62"). */
export const versionLabel = (version: string, detail?: string | null) =>
  version === HOMEBREW ? `Homebrew${detail ? ` ${detail}` : ""}` : version;

export type InstallPhase = "downloading" | "verifying" | "extracting" | "done";

export interface InstallProgress {
  tool: Tool;
  version: string;
  phase: InstallPhase;
  downloaded: number;
  total: number | null;
}

export interface InstallFinished {
  tool: Tool;
  version: string;
  /** Set when the install failed. */
  error: string | null;
}

export interface Platform {
  os: "windows" | "macos" | "linux";
  arch: "x64" | "arm64";
}

export const api = {
  getPlatform: () => invoke<Platform | null>("get_platform"),
  getState: () => invoke<AppState>("get_state"),
  getPaths: () => invoke<{ root: string }>("get_paths"),
  getCatalog: () => invoke<CatalogEntry[]>("get_catalog"),
  install: (tool: Tool, version: string) => invoke<void>("install_package", { tool, version }),
  uninstall: (tool: Tool, version: string) => invoke<void>("uninstall_package", { tool, version }),
  /** Moves an installed version to its newest patch release; returns the new version. */
  upgrade: (tool: Tool, version: string) => invoke<string>("upgrade_package", { tool, version }),
  onCatalogUpdated: (cb: () => void): Promise<UnlistenFn> => listen("catalog-updated", () => cb()),
  setActive: (tool: Tool, version: string) => invoke<void>("set_active", { tool, version }),
  serviceStatus: () => invoke<ServiceStatus[]>("service_status"),
  startService: (tool: Tool) => invoke<void>("start_service", { tool }),
  stopService: (tool: Tool) => invoke<void>("stop_service", { tool }),
  restartService: (tool: Tool) => invoke<void>("restart_service", { tool }),
  startAll: () => invoke<string[]>("start_all"),
  stopAll: () => invoke<void>("stop_all"),
  readLog: (tool: Tool) => invoke<string>("read_log", { tool }),
  getPhpConfig: () => invoke<PhpConfig>("get_php_config"),
  /** Returns `true` if PHP was restarted to apply the changes. */
  savePhpSettings: (settings: PhpSettings) => invoke<boolean>("save_php_settings", { settings }),
  getTools: () => invoke<ToolsInfo>("get_tools"),
  installExtra: (name: Extra) => invoke<void>("install_extra", { name }),
  uninstallExtra: (name: Extra) => invoke<void>("uninstall_extra", { name }),
  addShimsToPath: () => invoke<string[]>("add_shims_to_path"),
  openLocation: (what: Location) => invoke<void>("open_location", { what }),
  setPorts: (http: number, mariadb: number) => invoke<void>("set_ports", { http, mariadb }),
  mariadbInfo: () => invoke<MariadbInfo>("mariadb_info"),
  createDatabase: (name: string) => invoke<void>("create_database", { name }),
  getProjects: () => invoke<ProjectsInfo>("get_projects"),
  /** Adds a project or replaces `previous`. Returns `true` if Apache was restarted to apply it. */
  saveProject: (previous: string | null, project: Project) => invoke<boolean>("save_project", { previous, project }),
  /** Returns `true` if Apache was restarted to apply it. */
  removeProject: (name: string) => invoke<boolean>("remove_project", { name }),
  openProjectFolder: (name: string) => invoke<void>("open_project_folder", { name }),
  getTls: () => invoke<TlsInfo>("get_tls"),
  mailStatus: () => invoke<MailStatus>("mail_status"),
  startMail: () => invoke<void>("start_mail"),
  stopMail: () => invoke<void>("stop_mail"),
  /** Adds the local CA to the user's trust store; the system asks for confirmation. */
  trustCa: () => invoke<void>("trust_ca"),
  onInstallProgress: (cb: (p: InstallProgress) => void): Promise<UnlistenFn> =>
    listen<InstallProgress>("install-progress", (e) => cb(e.payload)),
  onInstallFinished: (cb: (f: InstallFinished) => void): Promise<UnlistenFn> =>
    listen<InstallFinished>("install-finished", (e) => cb(e.payload)),
  /** Installs running in the backend right now (they survive switching pages). */
  activeInstalls: () => invoke<InstallProgress[]>("active_installs"),
};

/** Command errors arrive from Rust as serialized strings. */
export const errorMessage = (e: unknown) => (typeof e === "string" ? e : String(e));

export const formatBytes = (n: number) =>
  n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`;
