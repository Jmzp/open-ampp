import { TOOL_LABELS, type AppState, type Tool } from "../lib/api";

const PORT: Record<Tool, (s: AppState) => number> = {
  apache: (s) => s.ports.http,
  php: (s) => s.ports.php_fcgi_base,
  mariadb: (s) => s.ports.mariadb,
};

export function Dashboard({ state, onManage }: { state: AppState; onManage: () => void }) {
  return (
    <section className="grid">
      {(Object.keys(TOOL_LABELS) as Tool[]).map((tool) => {
        const version = state.active[tool];
        return (
          <article key={tool} className="card">
            <div className="card-head">
              <span className="dot stopped" aria-label="detenido" />
              <h2>{TOOL_LABELS[tool]}</h2>
            </div>
            <dl>
              <dt>Versión</dt>
              <dd>{version ?? "No instalada"}</dd>
              <dt>Puerto</dt>
              <dd>{PORT[tool](state)}</dd>
            </dl>
            {version ? (
              <button disabled title="Disponible en la próxima fase">
                Iniciar
              </button>
            ) : (
              <button className="secondary" onClick={onManage}>
                Instalar una versión
              </button>
            )}
          </article>
        );
      })}
    </section>
  );
}
