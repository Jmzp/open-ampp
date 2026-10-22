import { useEffect, useState } from "react";
import {
  api,
  errorMessage,
  formatBytes,
  TOOL_LABELS,
  type CatalogEntry,
  type InstallProgress,
  type Tool,
} from "../lib/api";

const PHASE_LABELS = {
  downloading: "Descargando",
  verifying: "Verificando",
  extracting: "Extrayendo",
  done: "Listo",
} as const;

const key = (tool: Tool, version: string) => `${tool}@${version}`;

export function Versions({ onChanged }: { onChanged: () => void }) {
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [progress, setProgress] = useState<Record<string, InstallProgress>>({});
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  const refresh = () =>
    api
      .getCatalog()
      .then(setCatalog)
      .catch((e) => setError(errorMessage(e)));

  useEffect(() => {
    refresh();
    const unlisten = api.onInstallProgress((p) =>
      setProgress((prev) => ({ ...prev, [key(p.tool, p.version)]: p })),
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const run = async (entry: CatalogEntry, action: () => Promise<void>) => {
    const k = key(entry.tool, entry.version);
    setError(null);
    setBusy((s) => new Set(s).add(k));
    try {
      await action();
      await refresh();
      onChanged();
    } catch (e) {
      setError(`${TOOL_LABELS[entry.tool]} ${entry.version}: ${errorMessage(e)}`);
    } finally {
      setBusy((s) => {
        const next = new Set(s);
        next.delete(k);
        return next;
      });
      setProgress((prev) => {
        const { [k]: _, ...rest } = prev;
        return rest;
      });
    }
  };

  const tools = (Object.keys(TOOL_LABELS) as Tool[]).map((tool) => ({
    tool,
    entries: catalog.filter((e) => e.tool === tool),
  }));

  return (
    <section className="versions">
      {error && <p className="error">{error}</p>}
      {tools.map(({ tool, entries }) => (
        <div key={tool} className="card">
          <h2>{TOOL_LABELS[tool]}</h2>
          {entries.length === 0 ? (
            <p className="muted">Aún no hay versiones para tu sistema.</p>
          ) : (
            <ul className="rows">
              {entries.map((entry) => {
                const k = key(entry.tool, entry.version);
                const p = progress[k];
                const isBusy = busy.has(k);
                return (
                  <li key={k} className="row">
                    <span className="version">{entry.version}</span>
                    <span className="meta">
                      {entry.active && <span className="badge">Activa</span>}
                      {entry.installed && !entry.active && <span className="badge muted-badge">Instalada</span>}
                      {!entry.installed && entry.size != null && (
                        <span className="muted">{formatBytes(entry.size)}</span>
                      )}
                    </span>
                    {isBusy && p ? (
                      <ProgressBar progress={p} />
                    ) : (
                      <span className="actions">
                        {!entry.installed && (
                          <button disabled={isBusy} onClick={() => run(entry, () => api.install(tool, entry.version))}>
                            {isBusy ? "Preparando…" : "Instalar"}
                          </button>
                        )}
                        {entry.installed && !entry.active && (
                          <button
                            className="secondary"
                            disabled={isBusy}
                            onClick={() => run(entry, () => api.setActive(tool, entry.version))}
                          >
                            Usar por defecto
                          </button>
                        )}
                        {entry.installed && (
                          <button
                            className="ghost"
                            disabled={isBusy}
                            onClick={() => run(entry, () => api.uninstall(tool, entry.version))}
                          >
                            Desinstalar
                          </button>
                        )}
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      ))}
    </section>
  );
}

function ProgressBar({ progress }: { progress: InstallProgress }) {
  const pct =
    progress.phase === "downloading" && progress.total
      ? Math.round((progress.downloaded / progress.total) * 100)
      : null;
  return (
    <span className="progress">
      <span className="progress-label">
        {PHASE_LABELS[progress.phase]}
        {pct != null && ` ${pct}%`}
      </span>
      <span className="progress-track">
        <span
          className={pct == null ? "progress-fill indeterminate" : "progress-fill"}
          style={pct == null ? undefined : { width: `${pct}%` }}
        />
      </span>
    </span>
  );
}
