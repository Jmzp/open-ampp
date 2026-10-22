import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  errorMessage,
  TOOL_LABELS,
  versionLabel,
  type AppState,
  type ServiceStatus,
  type Tool,
} from "../lib/api";
import { usePolling } from "../lib/usePolling";

const TOOLS = Object.keys(TOOL_LABELS) as Tool[];

export function Dashboard({ state, onManage }: { state: AppState; onManage: () => void }) {
  const [statuses, setStatuses] = useState<ServiceStatus[]>([]);
  const [busy, setBusy] = useState<Set<Tool | "all">>(new Set());
  const [errors, setErrors] = useState<string[]>([]);

  const refresh = () => api.serviceStatus().then(setStatuses).catch(() => {});
  usePolling(refresh, 2000);

  const run = async (key: Tool | "all", action: () => Promise<unknown>) => {
    setBusy((s) => new Set(s).add(key));
    setErrors([]);
    try {
      const result = await action();
      if (Array.isArray(result)) setErrors(result as string[]);
    } catch (e) {
      setErrors([errorMessage(e)]);
    } finally {
      await refresh();
      setBusy((s) => {
        const next = new Set(s);
        next.delete(key);
        return next;
      });
    }
  };

  const status = (tool: Tool) => statuses.find((s) => s.tool === tool);
  const anyRunning = statuses.some((s) => s.running);
  const apacheUp = status("apache")?.running;
  const siteUrl = `http://localhost:${state.ports.http}`;

  return (
    <>
      <div className="toolbar">
        <button disabled={busy.has("all")} onClick={() => run("all", api.startAll)}>
          Iniciar todo
        </button>
        <button className="secondary" disabled={!anyRunning || busy.has("all")} onClick={() => run("all", api.stopAll)}>
          Detener todo
        </button>
        <button className="ghost" disabled={!apacheUp} onClick={() => openUrl(siteUrl)}>
          Abrir {siteUrl.replace("http://", "")} ↗
        </button>
      </div>

      {errors.map((e, i) => (
        <pre key={i} className="error-box">
          {e}
        </pre>
      ))}

      <section className="grid">
        {TOOLS.map((tool) => {
          const active = state.active[tool];
          const s = status(tool);
          const running = !!s?.running;
          const isBusy = busy.has(tool) || busy.has("all");
          return (
            <article key={tool} className="card">
              <div className="card-head">
                <span
                  className={`dot ${running ? "running" : s?.error ? "failed" : "stopped"}`}
                  aria-label={running ? "en ejecución" : "detenido"}
                />
                <h2>{TOOL_LABELS[tool]}</h2>
                <span className="muted state-label">
                  {isBusy ? "Trabajando…" : running ? "En ejecución" : "Detenido"}
                </span>
              </div>
              <dl>
                <dt>Versión</dt>
                <dd>{running ? s?.version : active ? versionLabel(active) : "No instalada"}</dd>
                <dt>Puerto</dt>
                <dd>{s?.port ?? "—"}</dd>
                <dt>PID</dt>
                <dd>{s?.pid ?? "—"}</dd>
              </dl>
              {s?.error && !running && <p className="error small">{s.error.split("\n")[0]}</p>}
              {!active ? (
                <button className="secondary" onClick={onManage}>
                  Instalar una versión
                </button>
              ) : running ? (
                <div className="actions">
                  <button className="secondary" disabled={isBusy} onClick={() => run(tool, () => api.restartService(tool))}>
                    Reiniciar
                  </button>
                  <button disabled={isBusy} onClick={() => run(tool, () => api.stopService(tool))}>
                    Detener
                  </button>
                </div>
              ) : (
                <button disabled={isBusy} onClick={() => run(tool, () => api.startService(tool))}>
                  Iniciar
                </button>
              )}
            </article>
          );
        })}
      </section>
    </>
  );
}
