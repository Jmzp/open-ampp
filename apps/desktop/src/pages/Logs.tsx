import { useEffect, useRef, useState } from "react";
import { api, TOOL_LABELS, type Tool } from "../lib/api";
import { usePolling } from "../lib/usePolling";

export function Logs() {
  const [tool, setTool] = useState<Tool>("apache");
  const [text, setText] = useState("");
  const [follow, setFollow] = useState(true);
  const pre = useRef<HTMLPreElement>(null);

  usePolling(() => api.readLog(tool).then(setText).catch(() => {}), 1500, [tool]);

  useEffect(() => {
    if (follow && pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [text, follow]);

  return (
    <section className="logs">
      <div className="toolbar">
        <nav className="tabs">
          {(Object.keys(TOOL_LABELS) as Tool[]).map((t) => (
            <button key={t} className={t === tool ? "tab active" : "tab"} onClick={() => setTool(t)}>
              {TOOL_LABELS[t]}
            </button>
          ))}
        </nav>
        <label className="muted check">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          Seguir el final
        </label>
      </div>
      <pre ref={pre} className="log-view">
        {text || "Sin registros todavía."}
      </pre>
    </section>
  );
}
