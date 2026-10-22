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
  version: string;
  size: number | null;
  installed: boolean;
  active: boolean;
}

export type InstallPhase = "downloading" | "verifying" | "extracting" | "done";

export interface InstallProgress {
  tool: Tool;
  version: string;
  phase: InstallPhase;
  downloaded: number;
  total: number | null;
}

export const api = {
  getState: () => invoke<AppState>("get_state"),
  getPaths: () => invoke<{ root: string }>("get_paths"),
  getCatalog: () => invoke<CatalogEntry[]>("get_catalog"),
  install: (tool: Tool, version: string) => invoke<void>("install_package", { tool, version }),
  uninstall: (tool: Tool, version: string) => invoke<void>("uninstall_package", { tool, version }),
  setActive: (tool: Tool, version: string) => invoke<void>("set_active", { tool, version }),
  onInstallProgress: (cb: (p: InstallProgress) => void): Promise<UnlistenFn> =>
    listen<InstallProgress>("install-progress", (e) => cb(e.payload)),
};

/** Los errores de comandos llegan como string serializado desde Rust. */
export const errorMessage = (e: unknown) => (typeof e === "string" ? e : String(e));

export const formatBytes = (n: number) =>
  n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`;
