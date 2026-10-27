import { useEffect, useMemo, useRef, useState } from "react";
import { api, type LogName } from "../lib/api";
import { t } from "../lib/i18n";
import { usePolling } from "../lib/usePolling";

const LOGS: { name: LogName; label: string }[] = [
  { name: "apache", label: "Apache" },
  { name: "apache-access", label: t.logs.access },
  { name: "php", label: "PHP" },
  { name: "mariadb", label: "MariaDB" },
  { name: "mailpit", label: "Mailpit" },
];

export function Logs() {
  const [name, setName] = useState<LogName>("apache");
  const [text, setText] = useState("");
  const [follow, setFollow] = useState(true);
  const [query, setQuery] = useState("");
  const pre = useRef<HTMLPreElement>(null);

  usePolling(() => api.readLog(name).then(setText).catch(() => {}), 1500, [name]);

  // Case-insensitive filter by line; the whole text when there is no query.
  const { shown, matches } = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return { shown: text, matches: null };
    const lines = text.split("\n").filter((l) => l.toLowerCase().includes(q));
    return { shown: lines.join("\n"), matches: lines.length };
  }, [text, query]);

  useEffect(() => {
    if (follow && pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [shown, follow]);

  return (
    <section className="logs">
      <div className="toolbar">
        <nav className="tabs">
          {LOGS.map((log) => (
            <button key={log.name} className={log.name === name ? "tab active" : "tab"} onClick={() => setName(log.name)}>
              {log.label}
            </button>
          ))}
        </nav>
        <label className="muted check">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          {t.logs.follow}
        </label>
      </div>
      <div className="log-search">
        <input
          type="search"
          value={query}
          placeholder={t.logs.search}
          onChange={(e) => setQuery(e.target.value)}
        />
        {matches !== null && <span className="muted small">{t.logs.matches(matches)}</span>}
        <button className="ghost" onClick={() => api.openLocation("logs")}>
          {t.logs.folder}
        </button>
      </div>
      <pre ref={pre} className="log-view">
        {shown || (matches === null ? t.logs.empty : t.logs.noMatches)}
      </pre>
    </section>
  );
}
