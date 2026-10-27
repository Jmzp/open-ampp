import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type AppState, type Extra, type ToolsInfo } from "../lib/api";
import { t } from "../lib/i18n";

const EXTRAS: Record<Extra, { title: string; description: string }> = {
  phpmyadmin: { title: "phpMyAdmin", description: t.tools.phpmyadmin },
  composer: { title: "Composer", description: t.tools.composer },
  mailpit: { title: "Mailpit", description: t.tools.mailpit },
};

const IS_WINDOWS = navigator.userAgent.includes("Windows");

export function Tools({ state, onChanged }: { state: AppState; onChanged: () => void }) {
  const [info, setInfo] = useState<ToolsInfo | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pathResult, setPathResult] = useState<string[] | null>(null);
  const [ports, setPorts] = useState({ http: String(state.ports.http), mariadb: String(state.ports.mariadb) });
  const [portsSaved, setPortsSaved] = useState(false);

  const load = () =>
    api
      .getTools()
      .then(setInfo)
      .catch((e) => setError(errorMessage(e)));

  useEffect(() => {
    load();
  }, []);

  const run = async (key: string, action: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await action();
      await load();
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  if (!info) return error ? <p className="error">{error}</p> : <p className="muted">{t.loading}</p>;

  const pmaUrl = `http://localhost:${state.ports.http}/phpmyadmin/`;
  const portsChanged = ports.http !== String(state.ports.http) || ports.mariadb !== String(state.ports.mariadb);

  return (
    <div className="stack">
      {error && <pre className="error-box">{error}</pre>}

      <section className="grid">
        {info.extras.map((e) => {
          const meta = EXTRAS[e.name];
          return (
            <article key={e.name} className="card">
              <div className="card-head">
                <span className={`dot ${e.installed ? "running" : "stopped"}`} />
                <h2>{meta.title}</h2>
                <span className="muted state-label">
                  {e.installed ? `v${e.installed}` : e.available ? t.tools.available(e.available) : t.tools.unavailable}
                </span>
              </div>
              <p className="muted small card-text">{meta.description}</p>
              {e.installed ? (
                <div className="actions">
                  {e.available && e.available !== e.installed && (
                    <button disabled={busy !== null} onClick={() => run(e.name, () => api.installExtra(e.name))}>
                      {busy === e.name ? t.tools.installing : t.tools.updateTo(e.available)}
                    </button>
                  )}
                  {e.name === "phpmyadmin" && (
                    <button onClick={() => openUrl(pmaUrl)} title={t.tools.needsServices}>
                      {t.tools.open}
                    </button>
                  )}
                  {e.name === "composer" && <code className="cmd">composer install</code>}
                  {e.name === "mailpit" && (
                    <button onClick={() => openUrl(`http://localhost:${state.ports.mail_web}/`)} title={t.tools.mailpitHint}>
                      {t.mail.open}
                    </button>
                  )}
                  <button
                    className="secondary"
                    disabled={busy !== null}
                    onClick={() => run(e.name, () => api.uninstallExtra(e.name))}
                  >
                    {t.tools.uninstall}
                  </button>
                </div>
              ) : (
                <button
                  disabled={busy !== null || !e.available}
                  onClick={() => run(e.name, () => api.installExtra(e.name))}
                >
                  {busy === e.name ? t.tools.installing : t.tools.install}
                </button>
              )}
            </article>
          );
        })}
      </section>

      <section className="panel">
        <div className="panel-head">
          <h3>{t.tools.terminal}</h3>
          {info.on_path ? (
            <span className="badge">{t.tools.onPath}</span>
          ) : (
            <button
              className="inline"
              disabled={busy !== null}
              onClick={() => run("path", async () => setPathResult(await api.addShimsToPath()))}
            >
              {t.tools.addToPath}
            </button>
          )}
        </div>
        <p className="muted small">{t.tools.shimsIntro}</p>
        <div className="chips">
          {info.commands.length === 0 && <span className="muted small">{t.tools.noShims}</span>}
          {info.commands.map((c) => (
            <code key={c} className="chip">
              {c}
            </code>
          ))}
        </div>
        {pathResult && <p className="ok-text small">{t.tools.pathDone(pathResult.join(", "))}</p>}
        <details className="small">
          <summary className="muted">{t.tools.manual}</summary>
          <pre className="snippet">
            {IS_WINDOWS ? `setx PATH "${info.shims_dir};%PATH%"` : `export PATH="${info.shims_dir}:$PATH"`}
          </pre>
        </details>
        <p className="muted small">
          {t.tools.cliIntro} <code>ampp up</code>, <code>ampp status</code>, <code>ampp install php</code>,{" "}
          <code>ampp upgrade</code>…
        </p>
      </section>

      <section className="panel">
        <h3>{t.tools.ports}</h3>
        <div className="form-grid">
          <label className="field">
            <span>{t.tools.httpPort}</span>
            <input
              inputMode="numeric"
              value={ports.http}
              onChange={(e) => setPorts({ ...ports, http: e.target.value.replace(/\D/g, "") })}
            />
          </label>
          <label className="field">
            <span>{t.tools.mariadbPort}</span>
            <input
              inputMode="numeric"
              value={ports.mariadb}
              onChange={(e) => setPorts({ ...ports, mariadb: e.target.value.replace(/\D/g, "") })}
            />
          </label>
        </div>
        <div className="save-row">
          <span className="muted small">{portsSaved ? t.tools.portsSaved : t.tools.portsHint}</span>
          <button
            className="inline"
            disabled={!portsChanged || busy !== null}
            onClick={() =>
              run("ports", async () => {
                await api.setPorts(Number(ports.http), Number(ports.mariadb));
                setPortsSaved(true);
              })
            }
          >
            {t.tools.savePorts}
          </button>
        </div>
      </section>

      <section className="panel">
        <h3>{t.tools.folders}</h3>
        <div className="actions">
          <button className="secondary" onClick={() => api.openLocation("www")}>
            {t.tools.www}
          </button>
          <button className="secondary" onClick={() => api.openLocation("logs")}>
            {t.tools.logs}
          </button>
          <button className="secondary" onClick={() => api.openLocation("data")}>
            {t.tools.data}
          </button>
        </div>
        <p className="muted small">{t.tools.trayHint}</p>
      </section>
    </div>
  );
}
