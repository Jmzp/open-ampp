import { useEffect, useState } from "react";
import { api, errorMessage, type PhpConfig, type PhpSettings } from "../lib/api";
import { t } from "../lib/i18n";

/** Directives editable from the form; labels and hints come from the i18n messages. */
const FIELDS: { key: string; options?: string[] }[] = [
  { key: "memory_limit" },
  { key: "upload_max_filesize" },
  { key: "post_max_size" },
  { key: "max_execution_time" },
  { key: "max_input_time" },
  { key: "max_input_vars" },
  { key: "display_errors", options: ["On", "Off"] },
  {
    key: "error_reporting",
    options: ["E_ALL", "E_ALL & ~E_DEPRECATED", "E_ALL & ~E_NOTICE & ~E_DEPRECATED", "E_ALL & ~E_DEPRECATED & ~E_STRICT"],
  },
  { key: "date.timezone" },
];

export function Php({ onManage }: { onManage: () => void }) {
  const [config, setConfig] = useState<PhpConfig | null>(null);
  const [draft, setDraft] = useState<PhpSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    api
      .getPhpConfig()
      .then((c) => {
        setConfig(c);
        setDraft(c.settings);
      })
      .catch((e) => setError(errorMessage(e)));

  useEffect(() => {
    load();
  }, []);

  if (error && !config) return <p className="error">{error}</p>;
  if (!config || !draft) return <p className="muted">{t.loading}</p>;
  if (!config.version) {
    return (
      <section className="panel">
        <p className="muted">{t.php.installFirst}</p>
        <button className="inline" onClick={onManage}>
          {t.php.goToVersions}
        </button>
      </section>
    );
  }

  const value = (key: string) => draft.directives[key] ?? "";
  const setValue = (key: string, v: string) => {
    const directives = { ...draft.directives };
    // Empty or equal to the base value means "use XAMPP's value".
    if (v.trim() === "" || v.trim() === config.defaults[key]) delete directives[key];
    else directives[key] = v.trim();
    setDraft({ ...draft, directives });
  };

  const enabledExtensions = draft.extensions ?? config.optional.filter((e) => e.enabled).map((e) => e.name);
  const toggleExtension = (name: string, on: boolean) => {
    const next = on ? [...enabledExtensions, name] : enabledExtensions.filter((e) => e !== name);
    setDraft({ ...draft, extensions: next });
  };

  const dirty = JSON.stringify(draft) !== JSON.stringify(config.settings);

  const save = async () => {
    setSaving(true);
    setMessage(null);
    setError(null);
    try {
      const restarted = await api.savePhpSettings(draft);
      setMessage(restarted ? t.php.savedRestarted : t.php.saved);
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="stack">
      <div className="toolbar">
        <h2 className="page-title">PHP {config.version}</h2>
        <button className="ghost" onClick={() => api.openLocation("php_ini")}>
          {t.php.viewIni}
        </button>
      </div>
      <p className="muted lead">{t.php.lead}</p>

      <section className="panel">
        <div className="panel-head">
          <h3>{t.php.common}</h3>
          <button
            className="ghost"
            disabled={Object.keys(draft.directives).length === 0}
            onClick={() => setDraft({ ...draft, directives: {} })}
          >
            {t.php.restoreXampp}
          </button>
        </div>
        <div className="form-grid">
          {FIELDS.map((f) => {
            const listId = f.options ? `opts-${f.key}` : undefined;
            const changed = f.key in draft.directives;
            const [label, hint] = t.php.fields[f.key] ?? [f.key, ""];
            return (
              <label key={f.key} className="field">
                <span>
                  {label}
                  {changed && <span className="badge">{t.php.modified}</span>}
                </span>
                <input
                  value={value(f.key)}
                  placeholder={config.defaults[f.key] ?? ""}
                  list={listId}
                  spellCheck={false}
                  onChange={(e) => setValue(f.key, e.target.value)}
                />
                {f.options && (
                  <datalist id={listId}>
                    {f.options.map((o) => (
                      <option key={o} value={o} />
                    ))}
                  </datalist>
                )}
                <small className="muted">
                  <code>{f.key}</code>
                  {hint ? ` · ${hint}` : ""}
                </small>
              </label>
            );
          })}
        </div>
      </section>

      <section className="panel">
        <div className="panel-head">
          <h3>{t.php.extensions}</h3>
          {config.optional.length > 0 && draft.extensions !== null && (
            <button className="ghost" onClick={() => setDraft({ ...draft, extensions: null })}>
              {t.php.restoreExtensions}
            </button>
          )}
        </div>
        {config.optional.length > 0 ? (
          <div className="ext-grid">
            {config.optional.map((e) => (
              <label key={e.name} className="check-item">
                <input
                  type="checkbox"
                  checked={enabledExtensions.includes(e.name)}
                  onChange={(ev) => toggleExtension(e.name, ev.target.checked)}
                />
                {e.name}
              </label>
            ))}
          </div>
        ) : (
          <>
            <p className="muted small">{t.php.builtIn}</p>
            <div className="chips">
              {config.loaded.map((m) => (
                <span key={m} className="chip">
                  {m}
                </span>
              ))}
            </div>
          </>
        )}
      </section>

      <section className="panel">
        <h3>{t.php.custom}</h3>
        <p className="muted small">{t.php.customHint}</p>
        <textarea
          className="code-input"
          rows={6}
          spellCheck={false}
          placeholder={t.php.customPlaceholder}
          value={draft.custom}
          onChange={(e) => setDraft({ ...draft, custom: e.target.value })}
        />
      </section>

      <div className="save-bar">
        {message && <span className="ok-text">{message}</span>}
        {error && <span className="error">{error}</span>}
        <button className="inline" disabled={!dirty || saving} onClick={save}>
          {saving ? t.php.saving : t.php.save}
        </button>
      </div>
    </div>
  );
}
