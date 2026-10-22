import { useEffect, useState } from "react";
import { api, type AppState, type Tool } from "./lib/api";
import "./App.css";

const SERVICES: { tool: Tool; label: string; port: (s: AppState) => number }[] = [
  { tool: "apache", label: "Apache", port: (s) => s.ports.http },
  { tool: "php", label: "PHP", port: (s) => s.ports.php_fcgi_base },
  { tool: "mariadb", label: "MariaDB", port: (s) => s.ports.mariadb },
];

function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [root, setRoot] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([api.getState(), api.getPaths()])
      .then(([s, p]) => {
        setState(s);
        setRoot(p.root);
      })
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <main className="app">
      <header>
        <h1>OPEN_AMPP</h1>
        <p className="muted">Apache · MariaDB · PHP para desarrollo local</p>
      </header>

      {error && <p className="error">{error}</p>}

      {state && (
        <section className="grid">
          {SERVICES.map(({ tool, label, port }) => {
            const version = state.active[tool];
            return (
              <article key={tool} className="card">
                <div className="card-head">
                  <span className="dot stopped" aria-label="detenido" />
                  <h2>{label}</h2>
                </div>
                <dl>
                  <dt>Versión</dt>
                  <dd>{version ?? "No instalada"}</dd>
                  <dt>Puerto</dt>
                  <dd>{port(state)}</dd>
                </dl>
                <button disabled={!version}>Iniciar</button>
              </article>
            );
          })}
        </section>
      )}

      {root && <footer className="muted">Datos en {root}</footer>}
    </main>
  );
}

export default App;
