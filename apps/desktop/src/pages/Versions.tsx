import { useEffect, useState } from "react";
import {
  api,
  errorMessage,
  formatBytes,
  TOOL_LABELS,
  type CatalogEntry,
  type InstallProgress,
  type Platform,
  type Tool,
  versionLabel,
} from "../lib/api";

const PHASE_LABELS = {
  downloading: "Descargando",
  verifying: "Verificando",
  extracting: "Extrayendo",
  done: "Listo",
} as const;

const HOMEBREW_FORMULA: Record<Tool, string> = { apache: "httpd", php: "php", mariadb: "mariadb" };

function EmptyHint({ tool, platform }: { tool: Tool; platform: Platform | null }) {
  if (platform?.os === "macos") {
    return (
      <p className="muted">
        Aún no hay binarios de {TOOL_LABELS[tool]} para macOS en el catálogo. Mientras tanto puedes instalarlo con
        Homebrew (<code>brew install {HOMEBREW_FORMULA[tool]}</code>) y reiniciar Open Ampp: se detecta solo.
      </p>
    );
  }
  return <p className="muted">Aún no hay versiones de {TOOL_LABELS[tool]} para tu sistema en el catálogo.</p>;
}

const key = (tool: Tool, version: string) => `${tool}@${version}`;

export function Versions({ onChanged }: { onChanged: () => void }) {
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [progress, setProgress] = useState<Record<string, InstallProgress>>({});
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [platform, setPlatform] = useState<Platform | null>(null);

  const refresh = () =>
    api
      .getCatalog()
      .then(setCatalog)
      .catch((e) => setError(errorMessage(e)));

  useEffect(() => {
    refresh();
    api.getPlatform().then(setPlatform);
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
            <EmptyHint tool={tool} platform={platform} />
          ) : (
            <ul className="rows">
              {entries.map((entry) => {
                const k = key(entry.tool, entry.version);
                const p = progress[k];
                const isBusy = busy.has(k);
                return (
                  <li key={k} className="row">
                    <span className="version">{versionLabel(entry.version, entry.detail)}</span>
                    <span className="meta">
                      {entry.active && <span className="badge">Activa</span>}
                      {entry.installed && !entry.active && <span className="badge muted-badge">Instalada</span>}
                      {entry.system && <span className="muted">Instalada en el sistema</span>}
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
                        {entry.installed && !entry.system && (
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
