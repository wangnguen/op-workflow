// The window's X with `onClose` = "ask": go to the tray (the app and the AI keep running) or quit
// (every running AI step is stopped together with what it started, then the app exits).
import { useEffect, useState } from "react";
import { vi } from "../i18n/vi";
import { closeWindow, errorMessage } from "../ipc/client";
import type { CloseChoice } from "../ipc/types";
import { Icon } from "../shared/ui/Icon";
import "../features/task/flowDialogs.css";

const c = vi.close;

interface Props {
  /** AI steps running when X was pressed */
  running: number;
  onClose: () => void;
}

export function CloseDialog({ running, onClose }: Props) {
  const [choice, setChoice] = useState<Exclude<CloseChoice, "ask">>("tray");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      if (!busy) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, onClose]);

  const go = () => {
    setBusy(true);
    setError(undefined);
    closeWindow(choice, remember)
      // the window is hidden now; the dialog must not be there when it comes back
      .then(onClose)
      .catch((e: unknown) => {
        setError(errorMessage(e));
        setBusy(false);
      });
  };

  const radio = (value: Exclude<CloseChoice, "ask">, title: string, hint: string) => (
    <label className={`flow-choice${choice === value ? " is-picked" : ""}`}>
      <input type="radio" name="close-choice" checked={choice === value} onChange={() => setChoice(value)} />
      <span className="flow-choice-text">
        <strong>{title}</strong>
        <small className="faint">{hint}</small>
      </span>
    </label>
  );

  return (
    <div className="scrim flow-scrim" onPointerDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="sheet flow-sheet" role="dialog" aria-modal="true" aria-labelledby="close-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="close" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="close-title">{c.title}</h2>
            <p>{running > 0 ? c.running(running) : c.idle}</p>
          </div>
        </header>

        <div className="sheet-body flow-body">
          <div className="flow-choices" role="radiogroup" aria-labelledby="close-title">
            {radio("tray", c.tray, c.trayHint)}
            {radio("quit", c.quit, running > 0 ? c.quitHint : c.quitIdleHint)}
          </div>
          <label className="flow-line close-remember">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
            <span>
              {c.remember}
              <small className="faint flow-fine">{c.rememberHint}</small>
            </span>
          </label>
          {error && <p className="note note--error">{error}</p>}
        </div>

        <footer className="sheet-foot flow-foot">
          <span className="flow-actions">
            <button className="btn btn--ghost btn--sm" disabled={busy} onClick={onClose}>
              {c.cancel}
            </button>
            <button
              className={`btn btn--sm close-go ${choice === "quit" ? "btn--danger" : "btn--primary"}`}
              disabled={busy}
              onClick={go}
              autoFocus
            >
              {choice === "quit" ? c.quit : c.tray}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
