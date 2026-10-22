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
import { t } from "../lib/i18n";

const HOMEBREW_FORMULA: Record<Tool, string> = { apache: "httpd", php: "php", mariadb: "mariadb" };

function EmptyHint({ tool, platform }: { tool: Tool; platform: Platform | null }) {
  if (platform?.os === "macos") {
    return <p className="muted">{t.versions.emptyMac(TOOL_LABELS[tool], HOMEBREW_FORMULA[tool])}</p>;
  }
  return <p className="muted">{t.versions.empty(TOOL_LABELS[tool])}</p>;
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

  const forget = (k: string) => {
    setBusy((s) => {
      const next = new Set(s);
      next.delete(k);
      return next;
    });
    setProgress((prev) => {
      const { [k]: _, ...rest } = prev;
      return rest;
    });
  };

  useEffect(() => {
    refresh();
    api.getPlatform().then(setPlatform);
    // Installs keep running in the backend while another page is shown: pick them up again.
    api.activeInstalls().then((running) => {
      setProgress((prev) => Object.fromEntries([...Object.entries(prev), ...running.map((p) => [key(p.tool, p.version), p])]));
      setBusy((prev) => new Set([...prev, ...running.map((p) => key(p.tool, p.version))]));
    });
    const unlisten = api.onInstallProgress((p) => {
      const k = key(p.tool, p.version);
      setProgress((prev) => ({ ...prev, [k]: p }));
      setBusy((prev) => (prev.has(k) ? prev : new Set(prev).add(k)));
    });
    const unlistenFinished = api.onInstallFinished((f) => {
      forget(key(f.tool, f.version));
      if (f.error) setError(`${TOOL_LABELS[f.tool]} ${f.version}: ${f.error}`);
      refresh();
      onChanged();
    });
    const unlistenCatalog = api.onCatalogUpdated(refresh);
    return () => {
      unlisten.then((f) => f());
      unlistenFinished.then((f) => f());
      unlistenCatalog.then((f) => f());
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
      forget(k);
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
                // An upgrade reports progress under the new version.
                const upgradeKey = entry.update ? key(entry.tool, entry.update) : null;
                const p = progress[k] ?? (upgradeKey ? progress[upgradeKey] : undefined);
                const isBusy = busy.has(k) || (upgradeKey !== null && busy.has(upgradeKey));
                return (
                  <li key={k} className="row">
                    <span className="version">{versionLabel(entry.version, entry.detail)}</span>
                    <span className="meta">
                      {entry.active && <span className="badge">{t.versions.active}</span>}
                      {entry.installed && !entry.active && <span className="badge muted-badge">{t.versions.installed}</span>}
                      {entry.system && <span className="muted">{t.versions.systemInstall}</span>}
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
                            {isBusy ? t.versions.preparing : t.versions.install}
                          </button>
                        )}
                        {entry.update && (
                          <button
                            title={t.versions.updateHint}
                            disabled={isBusy}
                            onClick={() =>
                              run(entry, async () => {
                                await api.upgrade(tool, entry.version);
                              })
                            }
                          >
                            {t.versions.updateTo(entry.update)}
                          </button>
                        )}
                        {entry.installed && !entry.active && (
                          <button
                            className="secondary"
                            disabled={isBusy}
                            onClick={() => run(entry, () => api.setActive(tool, entry.version))}
                          >
                            {t.versions.makeDefault}
                          </button>
                        )}
                        {entry.installed && !entry.system && (
                          <button
                            className="ghost"
                            disabled={isBusy}
                            onClick={() => run(entry, () => api.uninstall(tool, entry.version))}
                          >
                            {t.versions.uninstall}
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
        {t.versions.phases[progress.phase]}
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
