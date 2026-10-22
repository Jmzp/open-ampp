import { useCallback, useEffect, useState } from "react";
import { api, errorMessage, type AppState } from "./lib/api";
import { t } from "./lib/i18n";
import { UpdateBanner } from "./components/UpdateBanner";
import { Dashboard } from "./pages/Dashboard";
import { Logs } from "./pages/Logs";
import { Php } from "./pages/Php";
import { Tools } from "./pages/Tools";
import { Versions } from "./pages/Versions";
import "./App.css";

type Page = "dashboard" | "versions" | "php" | "tools" | "logs";

const PAGES: Page[] = ["dashboard", "versions", "php", "tools", "logs"];

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

  // Running installs, shown as a counter on the Versions tab from any page.
  const [installs, setInstalls] = useState<Set<string>>(new Set());
  useEffect(() => {
    const id = (tool: string, version: string) => `${tool}@${version}`;
    api.activeInstalls().then((running) => setInstalls(new Set(running.map((p) => id(p.tool, p.version)))));
    const unlisten = api.onInstallProgress((p) =>
      setInstalls((prev) => (prev.has(id(p.tool, p.version)) ? prev : new Set(prev).add(id(p.tool, p.version)))),
    );
    const unlistenFinished = api.onInstallFinished((f) =>
      setInstalls((prev) => {
        const next = new Set(prev);
        next.delete(id(f.tool, f.version));
        return next;
      }),
    );
    return () => {
      unlisten.then((f) => f());
      unlistenFinished.then((f) => f());
    };
  }, []);

  return (
    <main className="app">
      <header className="top">
        <div className="brand">
          <img src="/logo.svg" alt="" width={52} height={52} />
          <div>
            <h1>Open AMPP</h1>
            <p className="muted">{t.tagline}</p>
          </div>
        </div>
        <nav className="tabs">
          {PAGES.map((id) => (
            <button key={id} className={page === id ? "tab active" : "tab"} onClick={() => setPage(id)}>
              {t.tabs[id]}
              {id === "versions" && installs.size > 0 && <span className="tab-count">{installs.size}</span>}
            </button>
          ))}
        </nav>
      </header>

      <UpdateBanner />
      {error && <p className="error">{error}</p>}

      {page === "dashboard" && state && <Dashboard state={state} onManage={() => setPage("versions")} />}
      {page === "versions" && <Versions onChanged={reload} />}
      {page === "php" && <Php onManage={() => setPage("versions")} />}
      {page === "tools" && state && <Tools state={state} onChanged={reload} />}
      {page === "logs" && <Logs />}

      {root && <footer className="muted">{t.dataIn(root)}</footer>}
    </main>
  );
}

export default App;
