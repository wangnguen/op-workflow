// "Làm tiếp" (button, or a finished card dropped on Đang làm / Chờ push): carry on where the task
// stopped, go back to Viết code with a note, or run again from a step it already reached. The
// branch, brief, plan, findings and history stay; the task comes back ready, nothing runs yet.
import { useEffect, useState, type ReactNode } from "react";
import { stepLabel, vi } from "../../i18n/vi";
import { errorMessage, flowAction, getTaskDetail } from "../../ipc/client";
import type { FlowAction, StepKind, TaskColumn, TaskDetail } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { carryOnColumn, carryOnOptions, type CarryOnChoice } from "./carryOn";
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
  const [choice, setChoice] = useState<CarryOnChoice>();
  const [from, setFrom] = useState<StepKind>();
  const [note, setNote] = useState("");

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
  const picked = choice ?? options?.preferred;
  const fromStep = from ?? options?.from;

  const go = () => {
    if (!options || !picked) return;
    let action: FlowAction = { type: "reopen" };
    let step: StepKind | undefined = options.keep;
    if (picked === "implement") {
      action = { type: "reopen", step: "implement", note: note.trim() || undefined };
      step = "implement";
    } else if (picked === "from" && fromStep) {
      action = { type: "reopen", step: fromStep };
      step = fromStep;
    }
    setBusy(true);
    setError(undefined);
    flowAction(wpId, action)
      .then((d) => onDone(d, step, carryOnColumn(picked, options.keep)))
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  const radio = (value: CarryOnChoice, title: string, hint: string, extra?: ReactNode) => (
    <label className={`flow-choice${picked === value ? " is-picked" : ""}`}>
      <input type="radio" name="carry-on" checked={picked === value} onChange={() => setChoice(value)} />
      <span className="flow-choice-text">
        <strong>{title}</strong>
        <small className="faint">{hint}</small>
        {extra}
      </span>
    </label>
  );

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
          {options && (
            <div className="flow-choices" role="radiogroup" aria-labelledby="carry-title">
              {options.keep &&
                radio("keep", c.keep(stepLabel[options.keep]), options.keep === "pushSummary" ? c.keepPush : c.keepHere)}
              {options.implement &&
                radio(
                  "implement",
                  c.implement,
                  c.implementHint,
                  <textarea
                    className="input flow-note"
                    rows={3}
                    value={note}
                    placeholder={c.notePlaceholder}
                    onFocus={() => setChoice("implement")}
                    onChange={(e) => setNote(e.target.value)}
                  />,
                )}
              {options.steps.length > 0 &&
                radio(
                  "from",
                  c.from,
                  c.fromHint,
                  <select
                    className="input flow-select"
                    value={fromStep}
                    onFocus={() => setChoice("from")}
                    onChange={(e) => {
                      setChoice("from");
                      setFrom(e.target.value as StepKind);
                    }}
                  >
                    {options.steps.map((s) => (
                      <option key={s} value={s}>
                        {stepLabel[s]}
                      </option>
                    ))}
                  </select>,
                )}
            </div>
          )}
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
