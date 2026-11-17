// The "Làm tiếp" choices (carry on where it stopped / back to Viết code with a note / run
// again from a step), shared by the Làm tiếp dialog and "Bắt đầu làm việc" for a finished task.
import { useState, type ReactNode } from "react";
import { stepLabel, vi } from "../../i18n/vi";
import type { CarryOn, StepKind, TaskColumn } from "../../ipc/types";
import { carryOnColumn, type CarryOnChoice, type CarryOnOptions } from "./carryOn";
import "./flowDialogs.css";

const c = vi.carryOn;

export interface CarryOnPick {
  /** the Reopen fields */
  carryOn: CarryOn;
  /** step the task carries on at */
  step: StepKind | undefined;
  column: TaskColumn;
}

export function useCarryOn(options: CarryOnOptions | undefined) {
  const [choice, setChoice] = useState<CarryOnChoice>();
  const [from, setFrom] = useState<StepKind>();
  const [note, setNote] = useState("");
  const picked = choice ?? options?.preferred;
  const fromStep = from ?? options?.from;
  const pick = (): CarryOnPick | undefined => {
    if (!options || !picked) return undefined;
    const column = carryOnColumn(picked, options.keep);
    if (picked === "implement") return { carryOn: { step: "implement", note: note.trim() || undefined }, step: "implement", column };
    if (picked === "from" && fromStep) return { carryOn: { step: fromStep }, step: fromStep, column };
    return { carryOn: {}, step: options.keep, column };
  };
  return { picked, fromStep, note, setChoice, setFrom, setNote, pick };
}

export type CarryOnState = ReturnType<typeof useCarryOn>;

export function CarryOnFields({ options, state, labelledBy }: { options: CarryOnOptions; state: CarryOnState; labelledBy: string }) {
  const { picked, fromStep, note, setChoice, setFrom, setNote } = state;
  const radio = (value: CarryOnChoice, title: string, hint: string, extra?: ReactNode) => (
    <label className={`flow-choice${picked === value ? " is-picked" : ""}`}>
      <input type="radio" name={`carry-on-${labelledBy}`} checked={picked === value} onChange={() => setChoice(value)} />
      <span className="flow-choice-text">
        <strong>{title}</strong>
        <small className="faint">{hint}</small>
        {extra}
      </span>
    </label>
  );
  return (
    <div className="flow-choices" role="radiogroup" aria-labelledby={labelledBy}>
      {options.keep && radio("keep", c.keep(stepLabel[options.keep]), options.keep === "pushSummary" ? c.keepPush : c.keepHere)}
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
  );
}
