// "Làm tiếp" (button, or a finished card dropped on Đang làm / Chờ push): carry on where the task
// stopped, go back to Viết code with a note, or run again from a step it already reached. The
// branch, brief, plan, findings and history stay; the task comes back ready, nothing runs yet.
import { useEffect, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, flowAction, getTaskDetail } from "../../ipc/client";
import type { StepKind, TaskColumn, TaskDetail } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { carryOnOptions } from "./carryOn";
import { CarryOnFields, useCarryOn } from "./CarryOnFields";
import "./flowDialogs.css";

const c = vi.carryOn;

interface Props {
  wpId: number;
  taskId: string;
  /** the task window passes what it shows; the board lets the dialog read it */
  detail?: TaskDetail;
  /** column the card was dropped on */
  to?: TaskColumn;
  onClose: () => void;
  /** reopened: the new detail, the step it carries on at and the column it lands in */
  onDone: (detail: TaskDetail, step: StepKind | undefined, column: TaskColumn) => void;
}

export function ContinueDialog({ wpId, taskId, detail, to, onClose, onDone }: Props) {
  const [loaded, setLoaded] = useState<TaskDetail | undefined>(detail);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (detail) return;
    let live = true;
    getTaskDetail(wpId)
      .then((d) => live && setLoaded(d))
      .catch((e: unknown) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [wpId, detail]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      if (!busy) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, onClose]);

  const options = loaded ? carryOnOptions(loaded, to) : undefined;
  const carry = useCarryOn(options);

  const go = () => {
    const p = carry.pick();
    if (!p) return;
    setBusy(true);
    setError(undefined);
    flowAction(wpId, { type: "reopen", ...p.carryOn })
      .then((d) => onDone(d, p.step, p.column))
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="scrim flow-scrim" onPointerDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="sheet flow-sheet" role="dialog" aria-modal="true" aria-labelledby="carry-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="play" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="carry-title">{c.title}</h2>
            <p>
              <span className="mono">{taskId}</span>
              {loaded && ` · ${loaded.title}`}
            </p>
          </div>
        </header>

        <div className="sheet-body flow-body">
          {!loaded && !error && (
            <p className="flow-line flow-line--busy" role="status">
              <Icon name="refresh" size={14} className="is-spinning" />
              {c.loading}
            </p>
          )}
          {options && <CarryOnFields options={options} state={carry} labelledBy="carry-title" />}
          {error && <p className="note note--error">{error}</p>}
          {options && <small className="faint flow-fine">{c.keeps}</small>}
        </div>

        <footer className="sheet-foot flow-foot">
          <span className="flow-actions">
            <button className="btn btn--ghost btn--sm" disabled={busy} onClick={onClose}>
              {c.cancel}
            </button>
            <button className="btn btn--primary btn--sm" disabled={busy || !options} onClick={go}>
              <Icon name="play" size={12} />
              {c.confirm}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
