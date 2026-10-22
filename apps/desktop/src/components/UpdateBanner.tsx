import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { errorMessage } from "../lib/api";
import { t } from "../lib/i18n";

/** Checks GitHub Releases for a newer signed build once per launch (release builds only). */
export function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!import.meta.env.PROD) return;
    // Offline or no release published yet: nothing to show.
    check()
      .then(setUpdate)
      .catch(() => {});
  }, []);

  if (!update) return null;

  const install = async () => {
    setError(null);
    let total = 0;
    let done = 0;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") total = event.data.contentLength ?? 0;
        if (event.event === "Progress") {
          done += event.data.chunkLength;
          setProgress(total ? Math.round((done / total) * 100) : 0);
        }
      });
      await relaunch();
    } catch (e) {
      setError(errorMessage(e));
      setProgress(null);
    }
  };

  return (
    <div className="banner">
      <span>
        <strong>{t.update.available(update.version)}</strong>
        {error && <span className="error"> {error}</span>}
      </span>
      <button className="inline" disabled={progress !== null} onClick={install}>
        {progress === null ? t.update.install : t.update.downloading(progress)}
      </button>
    </div>
  );
}
