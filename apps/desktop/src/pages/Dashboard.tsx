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
import { DatabasePanel } from "../components/DatabasePanel";
import { t } from "../lib/i18n";
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
          {t.dashboard.startAll}
        </button>
        <button className="secondary" disabled={!anyRunning || busy.has("all")} onClick={() => run("all", api.stopAll)}>
          {t.dashboard.stopAll}
        </button>
        <button className="ghost" disabled={!apacheUp} onClick={() => openUrl(siteUrl)}>
          {t.dashboard.open(siteUrl.replace("http://", ""))}
        </button>
        <button className="ghost" onClick={() => api.openLocation("www")}>
          {t.dashboard.wwwFolder}
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
                  aria-label={running ? t.dashboard.running : t.dashboard.stopped}
                />
                <h2>{TOOL_LABELS[tool]}</h2>
                <span className="muted state-label">
                  {isBusy
                    ? t.dashboard.working
                    : running
                      ? s?.external
                        ? t.dashboard.runningTerminal
                        : t.dashboard.running
                      : t.dashboard.stopped}
                </span>
              </div>
              <dl>
                <dt>{t.dashboard.version}</dt>
                <dd>{running ? s?.version : active ? versionLabel(active) : t.dashboard.notInstalled}</dd>
                <dt>{t.dashboard.port}</dt>
                <dd>{s?.port ?? "—"}</dd>
                <dt>{t.dashboard.pid}</dt>
                <dd>{s?.pid ?? "—"}</dd>
              </dl>
              {running && !!s?.extra_versions.length && (
                <p className="muted small">{t.dashboard.alsoRunning(s.extra_versions.join(", "))}</p>
              )}
              {s?.error && !running && <p className="error small">{s.error.split("\n")[0]}</p>}
              {!active ? (
                <button className="secondary" onClick={onManage}>
                  {t.dashboard.installVersion}
                </button>
              ) : running ? (
                <div className="actions">
                  <button className="secondary" disabled={isBusy} onClick={() => run(tool, () => api.restartService(tool))}>
                    {t.dashboard.restart}
                  </button>
                  <button disabled={isBusy} onClick={() => run(tool, () => api.stopService(tool))}>
                    {t.dashboard.stop}
                  </button>
                </div>
              ) : (
                <button disabled={isBusy} onClick={() => run(tool, () => api.startService(tool))}>
                  {t.dashboard.start}
                </button>
              )}
            </article>
          );
        })}
      </section>

      <DatabasePanel state={state} running={!!status("mariadb")?.running} />
    </>
  );
}
