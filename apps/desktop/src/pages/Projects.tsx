import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type Project, type ProjectInfo, type ProjectsInfo, type TlsInfo } from "../lib/api";
import { t } from "../lib/i18n";

/** Same rule as the Rust side (`projects::suggested_domain`): "My Blog" → "my-blog.localhost". */
const suggestedDomain = (name: string) => {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `${slug || "site"}.localhost`;
};

const EMPTY: Project = { name: "", domain: "", docroot: "", php: null, https: false };

/** Project being edited: `previous` is its saved name, `null` for a new one. */
interface Draft {
  previous: string | null;
  project: Project;
  /** The domain was typed by hand, so it no longer follows the name. */
  customDomain: boolean;
}

export function Projects({ onChanged }: { onChanged: () => void }) {
  const [info, setInfo] = useState<ProjectsInfo | null>(null);
  const [tls, setTls] = useState<TlsInfo | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = () =>
    Promise.all([api.getProjects().then(setInfo), api.getTls().then(setTls)]).catch((e) => setError(errorMessage(e)));

  useEffect(() => {
    load();
  }, []);

  const run = async (action: () => Promise<string>) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setNotice(await action());
      await load();
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const save = (d: Draft) =>
    run(async () => {
      // HTTPS only applies to .localhost domains; drop it if the domain changed to another one.
      const project = { ...d.project, https: d.project.https && d.project.domain.trim().toLowerCase().endsWith(".localhost") };
      const restarted = await api.saveProject(d.previous, project);
      setDraft(null);
      return restarted ? t.projects.savedRestarted : t.projects.saved;
    });

  const remove = (name: string) =>
    run(async () => {
      await api.removeProject(name);
      return t.projects.removed;
    });

  const edit = (p: ProjectInfo) =>
    setDraft({
      previous: p.name,
      project: { name: p.name, domain: p.domain, docroot: p.docroot, php: p.php, https: p.https },
      customDomain: p.domain !== suggestedDomain(p.name),
    });

  if (!info) return error ? <p className="error">{error}</p> : <p className="muted">{t.loading}</p>;

  return (
    <div className="stack">
      <div className="toolbar">
        <h2 className="page-title">{t.projects.title}</h2>
        {!draft && (
          <button
            className="inline"
            onClick={() => {
              setNotice(null);
              setDraft({ previous: null, project: EMPTY, customDomain: false });
            }}
          >
            {t.projects.add}
          </button>
        )}
      </div>
      <p className="muted lead">{t.projects.lead}</p>

      {error && <pre className="error-box">{error}</pre>}
      {notice && <p className="ok-text small">{notice}</p>}

      {draft && (
        <ProjectForm
          draft={draft}
          info={info}
          httpsPort={tls?.https_port ?? 8443}
          busy={busy}
          onChange={setDraft}
          onSave={() => save(draft)}
          onCancel={() => {
            setDraft(null);
            setError(null);
          }}
        />
      )}

      <section className="panel">
        {info.projects.length === 0 ? (
          <p className="muted small">{t.projects.empty}</p>
        ) : (
          <ul className="project-list">
            {info.projects.map((p) => (
              <li key={p.name} className="project">
                <div className="project-main">
                  <div className="project-title">
                    <strong>{p.name}</strong>
                    <a
                      href={p.url}
                      onClick={(e) => {
                        e.preventDefault();
                        openUrl(p.url);
                      }}
                    >
                      {p.url.replace("http://", "").replace(/\/$/, "")}
                    </a>
                    <span className="chip">PHP {p.php_version ?? info.active_series ?? "—"}</span>
                    {p.https && <span className="chip">HTTPS</span>}
                  </div>
                  <span className="muted small project-path" title={p.docroot}>
                    {p.docroot}
                  </span>
                  {p.php && !p.php_version && <span className="warn-text small">{t.projects.phpMissing(p.php)}</span>}
                  {!p.folder_exists && <span className="warn-text small">{t.projects.folderMissing}</span>}
                </div>
                <div className="actions">
                  <button onClick={() => openUrl(p.url)}>{t.projects.open}</button>
                  <button className="secondary" disabled={!p.folder_exists} onClick={() => api.openProjectFolder(p.name)}>
                    {t.projects.openFolder}
                  </button>
                  <button className="secondary" disabled={busy || !!draft} onClick={() => edit(p)}>
                    {t.projects.editShort}
                  </button>
                  <button className="ghost" disabled={busy} title={t.projects.removeHint} onClick={() => remove(p.name)}>
                    {t.projects.remove}
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      {tls?.used && <TlsPanel tls={tls} onChanged={load} />}
    </div>
  );
}

function TlsPanel({ tls, onChanged }: { tls: TlsInfo; onChanged: () => Promise<unknown> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  const trust = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.trustCa();
      setDone(true);
      await onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const status =
    tls.trusted === null ? (
      <span className="badge muted-badge">{t.projects.tls.unknown}</span>
    ) : tls.trusted ? (
      <span className="badge">{t.projects.tls.trusted}</span>
    ) : (
      <span className="badge warn-badge">{t.projects.tls.notTrusted}</span>
    );

  return (
    <section className="panel">
      <div className="panel-head">
        <h3>{t.projects.tls.title}</h3>
        {status}
      </div>
      <p className="muted small">{t.projects.tls.intro}</p>
      {error && <pre className="error-box">{error}</pre>}
      {done && <p className="ok-text small">{t.projects.tls.trustedDone}</p>}
      {tls.linux_commands ? (
        <>
          <p className="muted small">{t.projects.tls.linux}</p>
          <pre className="snippet">{tls.linux_commands}</pre>
        </>
      ) : (
        tls.trusted === false && (
          <>
            <p className="muted small">{t.projects.tls.untrustedHint}</p>
            <div className="save-row">
              <span className="muted small">{t.projects.tls.trustHint}</span>
              <button className="inline" disabled={busy} onClick={trust}>
                {busy ? t.projects.tls.trusting : t.projects.tls.trust}
              </button>
            </div>
          </>
        )
      )}
      <p className="muted small">{t.projects.tls.firefox}</p>
      <div className="actions">
        <button className="secondary" onClick={() => api.openLocation("certs")}>
          {t.projects.tls.folder}
        </button>
      </div>
    </section>
  );
}

function ProjectForm({
  draft,
  info,
  httpsPort,
  busy,
  onChange,
  onSave,
  onCancel,
}: {
  draft: Draft;
  info: ProjectsInfo;
  httpsPort: number;
  busy: boolean;
  onChange: (d: Draft) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const { project } = draft;
  const set = (patch: Partial<Project>, customDomain = draft.customDomain) =>
    onChange({ ...draft, project: { ...project, ...patch }, customDomain });

  const chooseFolder = async () => {
    const dir = await open({ directory: true, defaultPath: project.docroot || undefined });
    if (typeof dir !== "string") return;
    const patch: Partial<Project> = { docroot: dir };
    // Name the project after its folder (the parent one for Laravel's public/).
    if (!project.name) {
      const parts = dir.split(/[\\/]/).filter(Boolean);
      const last = parts[parts.length - 1] ?? "";
      const name = last === "public" && parts.length > 1 ? parts[parts.length - 2] : last;
      patch.name = name;
      if (!draft.customDomain) patch.domain = suggestedDomain(name);
    }
    set(patch);
  };

  const domain = project.domain.trim().toLowerCase();
  const complete = project.name.trim() && domain && project.docroot;
  // Same rule as the Rust side (`projects::https_allowed`): the local CA only signs .localhost.
  const httpsAllowed = domain.endsWith(".localhost");

  return (
    <section className="panel">
      <h3>{draft.previous ? t.projects.edit : t.projects.add}</h3>
      <div className="form-grid">
        <label className="field">
          <span>{t.projects.name}</span>
          <input
            value={project.name}
            placeholder="blog"
            onChange={(e) => {
              const name = e.target.value;
              set(draft.customDomain ? { name } : { name, domain: suggestedDomain(name) });
            }}
          />
        </label>
        <label className="field">
          <span>{t.projects.domain}</span>
          <input
            value={project.domain}
            placeholder="blog.localhost"
            onChange={(e) => set({ domain: e.target.value }, true)}
          />
          {domain && !domain.endsWith(".localhost") ? (
            <small className="warn-text">{t.projects.hostsHint}</small>
          ) : (
            <small className="muted">{t.projects.domainHint}</small>
          )}
        </label>
        <label className="field">
          <span>{t.projects.folder}</span>
          <div className="picker">
            <input value={project.docroot} onChange={(e) => set({ docroot: e.target.value })} />
            <button type="button" className="secondary inline" onClick={chooseFolder}>
              {t.projects.choose}
            </button>
          </div>
          <small className="muted">{t.projects.folderHint}</small>
        </label>
        <label className="field">
          <span>{t.projects.php}</span>
          <select value={project.php ?? ""} onChange={(e) => set({ php: e.target.value || null })}>
            <option value="">{t.projects.activePhp(info.active_series)}</option>
            {info.php_series.map((s) => (
              <option key={s} value={s}>
                PHP {s}
              </option>
            ))}
            {project.php && !info.php_series.includes(project.php) && (
              <option value={project.php}>PHP {project.php}</option>
            )}
          </select>
        </label>
        <label className="check-field">
          <input
            type="checkbox"
            checked={project.https && httpsAllowed}
            disabled={!httpsAllowed}
            onChange={(e) => set({ https: e.target.checked })}
          />
          <span>
            {t.projects.https}
            <small className="muted">{httpsAllowed ? t.projects.httpsHint(httpsPort) : t.projects.httpsOnlyLocalhost}</small>
          </span>
        </label>
      </div>
      <div className="save-row">
        <span />
        <div className="actions">
          <button className="secondary" disabled={busy} onClick={onCancel}>
            {t.projects.cancel}
          </button>
          <button disabled={busy || !complete} onClick={onSave}>
            {busy ? t.projects.saving : t.projects.save}
          </button>
        </div>
      </div>
    </section>
  );
}
