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
}

export interface Project {
  name: string;
  domain: string;
  docroot: string;
  php: string | null;
  https: boolean;
}

export interface AppState {
  installed: Partial<Record<Tool, string[]>>;
  active: Partial<Record<Tool, string>>;
  ports: Ports;
  projects: Project[];
}

export interface CatalogEntry {
  tool: Tool;
  /** Identificador: "8.3.12" o "homebrew". */
  version: string;
  /** Versión real cuando difiere del identificador (Homebrew). */
  detail: string | null;
  size: number | null;
  installed: boolean;
  active: boolean;
  system: boolean;
}

export interface ServiceStatus {
  tool: Tool;
  running: boolean;
  pid: number | null;
  version: string | null;
  port: number | null;
  error: string | null;
}

export const HOMEBREW = "homebrew";

/** Texto para mostrar una versión ("homebrew" → "Homebrew 2.4.62"). */
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
  setActive: (tool: Tool, version: string) => invoke<void>("set_active", { tool, version }),
  serviceStatus: () => invoke<ServiceStatus[]>("service_status"),
  startService: (tool: Tool) => invoke<void>("start_service", { tool }),
  stopService: (tool: Tool) => invoke<void>("stop_service", { tool }),
  restartService: (tool: Tool) => invoke<void>("restart_service", { tool }),
  startAll: () => invoke<string[]>("start_all"),
  stopAll: () => invoke<void>("stop_all"),
  readLog: (tool: Tool) => invoke<string>("read_log", { tool }),
  onInstallProgress: (cb: (p: InstallProgress) => void): Promise<UnlistenFn> =>
    listen<InstallProgress>("install-progress", (e) => cb(e.payload)),
};

/** Los errores de comandos llegan como string serializado desde Rust. */
export const errorMessage = (e: unknown) => (typeof e === "string" ? e : String(e));

export const formatBytes = (n: number) =>
  n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`;
