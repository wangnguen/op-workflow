import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { downloadUpdate, errorCode, errorMessage, installUpdate } from "../../ipc/client";
import type { UpdateView } from "../../ipc/types";
import "./update.css";

const u = vi.update;
/** How often the state is read again while the installer downloads. */
const PROGRESS_MS = 1000;

/** Percent downloaded, when the size is known. */
export function downloadPct(update: UpdateView): number | undefined {
  const d = update.download;
  if (!d?.total) return undefined;
  return Math.min(100, Math.round((d.done / d.total) * 100));
}

interface Props {
  update: UpdateView;
  /** read the update state again (the progress, the result) */
  onChanged: () => void;
  onOpenRelease: () => void;
}

/** A newer version: download it (the app usually has already, in the background), then one click
 *  installs it. The installer closes the app, installs without questions and opens it again. */
export function UpdateActions({ update, onChanged, onOpenRelease }: Props) {
  const [busy, setBusy] = useState<string>();
  const [failure, setFailure] = useState<{ message: string; running: boolean }>();
  const d = update.download;
  const v = update.latest ?? "";
  const pct = downloadPct(update);
  const downloading = d?.state === "downloading";

  // the progress follows while the installer downloads (the parent's callback changes on every
  // render, so the timer reads the latest one instead of restarting each time)
  const changed = useRef(onChanged);
  changed.current = onChanged;
  useEffect(() => {
    if (!downloading) return;
    const id = window.setInterval(() => changed.current(), PROGRESS_MS);
    return () => window.clearInterval(id);
  }, [downloading]);

  const run = async (what: string, call: () => Promise<unknown>) => {
    setBusy(what);
    setFailure(undefined);
    try {
      await call();
      onChanged();
    } catch (e) {
      setFailure({ message: errorMessage(e), running: errorCode(e) === "update.running" });
    } finally {
      setBusy(undefined);
    }
  };

  let status: React.ReactNode;
  if (!update.canInstall) status = <p className="dim">{u.noInstaller}</p>;
  else if (downloading)
    status = (
      <>
        <div className="update-bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
          <span style={{ width: `${pct ?? 8}%` }} />
        </div>
        <p className="faint">{u.downloading(pct)}</p>
      </>
    );
  else if (d?.state === "ready") status = <p className="dim">{u.ready}</p>;
  else if (d?.state === "failed") status = <p className="note note--error">{u.failed(d.error ?? "")}</p>;
  else status = <p className="dim">{u.notYet}</p>;

  return (
    <div className="update-actions">
      <p className="update-title">{u.title(v, update.current)}</p>
      {status}
      {failure && (
        <div className="note note--error update-failure">
          <span>{failure.message}</span>
          {failure.running && (
            <button className="btn btn--sm" disabled={busy !== undefined} onClick={() => void run("install", () => installUpdate(true))}>
              {u.installAnyway}
            </button>
          )}
        </div>
      )}
      <div className="update-buttons">
        {update.canInstall && d?.state === "ready" && (
          <button className="btn btn--primary btn--sm" disabled={busy !== undefined} title={u.installHint}
            onClick={() => void run("install", () => installUpdate(false))}>
            {busy === "install" ? u.installing : u.install}
          </button>
        )}
        {update.canInstall && (!d || d.state === "failed") && (
          <button className="btn btn--primary btn--sm" disabled={busy !== undefined}
            onClick={() => void run("download", () => downloadUpdate())}>
            {d?.state === "failed" ? u.retry : u.download}
          </button>
        )}
        <button className="btn btn--ghost btn--sm" onClick={onOpenRelease}>
          {u.notes}
        </button>
      </div>
      {update.canInstall && <p className="faint update-hint">{u.installHint}</p>}
    </div>
  );
}
