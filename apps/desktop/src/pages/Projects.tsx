import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type Project, type ProjectInfo, type ProjectsInfo } from "../lib/api";
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
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = () =>
    api
      .getProjects()
      .then(setInfo)
      .catch((e) => setError(errorMessage(e)));

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
      const restarted = await api.saveProject(d.previous, d.project);
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
    </div>
  );
}

function ProjectForm({
  draft,
  info,
  busy,
  onChange,
  onSave,
  onCancel,
}: {
  draft: Draft;
  info: ProjectsInfo;
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
