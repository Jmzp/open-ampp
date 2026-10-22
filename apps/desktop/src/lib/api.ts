import { invoke } from "@tauri-apps/api/core";

export type Tool = "apache" | "php" | "mariadb";

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

export const api = {
  getState: () => invoke<AppState>("get_state"),
  getPaths: () => invoke<{ root: string }>("get_paths"),
};
