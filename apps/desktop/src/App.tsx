import { useCallback, useEffect, useState } from "react";
import { api, errorMessage, type AppState } from "./lib/api";
import { Dashboard } from "./pages/Dashboard";
import { Logs } from "./pages/Logs";
import { Versions } from "./pages/Versions";
import "./App.css";

type Page = "dashboard" | "versions" | "logs";

const PAGES: { id: Page; label: string }[] = [
  { id: "dashboard", label: "Panel" },
  { id: "versions", label: "Versiones" },
  { id: "logs", label: "Logs" },
];

function App() {
  const [page, setPage] = useState<Page>("dashboard");
  const [state, setState] = useState<AppState | null>(null);
  const [root, setRoot] = useState("");
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(
    () =>
      api
        .getState()
        .then(setState)
        .catch((e) => setError(errorMessage(e))),
    [],
  );

  useEffect(() => {
    reload();
    api.getPaths().then((p) => setRoot(p.root));
  }, [reload]);

  return (
    <main className="app">
      <header className="top">
        <div>
          <h1>OPEN_AMPP</h1>
          <p className="muted">Apache · MariaDB · PHP para desarrollo local</p>
        </div>
        <nav className="tabs">
          {PAGES.map(({ id, label }) => (
            <button key={id} className={page === id ? "tab active" : "tab"} onClick={() => setPage(id)}>
              {label}
            </button>
          ))}
        </nav>
      </header>

      {error && <p className="error">{error}</p>}

      {page === "dashboard" && state && <Dashboard state={state} onManage={() => setPage("versions")} />}
      {page === "versions" && <Versions onChanged={reload} />}
      {page === "logs" && <Logs />}

      {root && <footer className="muted">Datos en {root}</footer>}
    </main>
  );
}

export default App;
