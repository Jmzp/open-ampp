import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, formatBytes, type AppState, type MariadbInfo } from "../lib/api";
import { t } from "../lib/i18n";
import { usePolling } from "../lib/usePolling";

/** MariaDB credentials to copy, plus the databases on the running server. */
export function DatabasePanel({ state, running }: { state: AppState; running: boolean }) {
  const [info, setInfo] = useState<MariadbInfo | null>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [showSystem, setShowSystem] = useState(false);

  const load = () => api.mariadbInfo().then(setInfo).catch((e) => setError(errorMessage(e)));
  usePolling(load, 5000, [running]);

  if (!info || !state.active.mariadb) return null;
  const c = info.connection;

  const copy = (label: string, value: string) => {
    navigator.clipboard.writeText(value);
    setCopied(label);
    setTimeout(() => setCopied(null), 1500);
  };

  const create = async () => {
    setError(null);
    try {
      await api.createDatabase(name.trim());
      setName("");
      await load();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const env = `DB_CONNECTION=mariadb\nDB_HOST=${c.host}\nDB_PORT=${c.port}\nDB_DATABASE=${name.trim() || "my_app"}\nDB_USERNAME=${c.user}\nDB_PASSWORD=`;
  const pma = state.extras.phpmyadmin
    ? (db: string) => `http://localhost:${state.ports.http}/phpmyadmin/index.php?route=/database/structure&db=${encodeURIComponent(db)}`
    : null;
  const rows: [string, string, string][] = [
    ["host", t.database.host, c.host],
    ["port", t.database.port, String(c.port)],
    ["user", t.database.user, c.user],
    ["password", t.database.password, ""],
    ...(c.socket ? ([["socket", t.database.socket, c.socket]] as [string, string, string][]) : []),
  ];
  const visible = info.databases?.filter((d) => showSystem || !d.system) ?? [];

  return (
    <section className="panel">
      <h3>{t.database.title}</h3>
      <div className="db-layout">
        <div>
          <dl className="creds">
            {rows.map(([id, label, value]) => (
              <div key={id} className="cred">
                <dt>{label}</dt>
                <dd>
                  <code>{value || t.database.empty}</code>
                  {value && (
                    <button className="ghost copy" onClick={() => copy(id, value)}>
                      {copied === id ? t.database.copied : t.database.copy}
                    </button>
                  )}
                </dd>
              </div>
            ))}
          </dl>
          <details className="small">
            <summary className="muted">{t.database.example}</summary>
            <pre className="snippet">{env}</pre>
            <button className="ghost copy" onClick={() => copy("env", env)}>
              {copied === "env" ? t.database.copied : t.database.copy}
            </button>
          </details>
        </div>

        <div>
          <div className="panel-head">
            <strong>{t.database.databases}</strong>
            {info.databases && (
              <label className="muted check small">
                <input type="checkbox" checked={showSystem} onChange={(e) => setShowSystem(e.target.checked)} />
                {t.database.showSystem}
              </label>
            )}
          </div>
          {!info.databases ? (
            <p className="muted small">{running ? t.loading : t.database.stopped}</p>
          ) : (
            <>
              {visible.length === 0 && <p className="muted small">{t.database.noUserDbs}</p>}
              <ul className="db-list">
                {visible.map((d) => (
                  <li key={d.name}>
                    <span className="db-name">{d.name}</span>
                    {d.system && <span className="badge muted-badge">{t.database.system}</span>}
                    <span className="muted small">
                      {t.database.tables(d.tables)} · {formatBytes(d.size)}
                    </span>
                    {pma && (
                      <button className="ghost copy" onClick={() => openUrl(pma(d.name))} title={t.database.openPma}>
                        ↗
                      </button>
                    )}
                  </li>
                ))}
              </ul>
              <form
                className="db-create"
                onSubmit={(e) => {
                  e.preventDefault();
                  create();
                }}
              >
                <input
                  value={name}
                  placeholder={t.database.newPlaceholder}
                  spellCheck={false}
                  onChange={(e) => setName(e.target.value.replace(/[^A-Za-z0-9_-]/g, ""))}
                />
                <button className="inline" disabled={!name.trim()}>
                  {t.database.create}
                </button>
              </form>
            </>
          )}
          {error && <p className="error small">{error}</p>}
        </div>
      </div>
    </section>
  );
}
