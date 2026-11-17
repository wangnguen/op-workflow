// Plan tab extras: the analysis's questions (answered at the plan gate, Duyệt waits for them)
// and the optional steps it advises (Harness, Test trình duyệt), switched per task.
import { useState } from "react";
import type { FlowAction, PlanQuestion, StepKind, TaskDetail } from "../../ipc/types";
import { stepLabel, vi } from "../../i18n/vi";
import { Icon } from "../../shared/ui/Icon";
import { DevServerBox } from "./DevServerBox";
import { unanswered } from "./planQuestions";

const t = vi.flow;
const SWITCHABLE: StepKind[] = ["harness", "browserTest"];

interface Props {
  detail: TaskDetail;
  onAction: (a: FlowAction) => void;
}

/** One card per question: the AI's suggested answers, a free answer, or "AI tự quyết". */
export function PlanQuestions({ detail: d, onAction }: Props) {
  const questions = d.questions ?? [];
  if (questions.length === 0) return null;
  const left = unanswered(questions);
  return (
    <section className="plan-questions" aria-label={t.questions.title}>
      <h4>
        {t.questions.title}
        <span className={left > 0 ? "plan-left plan-left--open" : "plan-left"}> · {t.questions.left(left)}</span>
      </h4>
      <ol>
        {questions.map((q) => (
          <QuestionRow key={q.id} q={q} onAction={onAction} />
        ))}
      </ol>
    </section>
  );
}

function QuestionRow({ q, onAction }: { q: PlanQuestion; onAction: (a: FlowAction) => void }) {
  const saved = q.answer ?? "";
  const options = q.options ?? [];
  const typed = options.includes(saved) ? "" : saved;
  // what the user is typing; undefined = show the saved answer
  const [draft, setDraft] = useState<string>();
  const value = draft ?? typed;
  const answer = (text: string) => {
    setDraft(undefined);
    onAction({ type: "answerQuestion", id: q.id, answer: text });
  };
  const save = () => {
    if (draft !== undefined && draft.trim() !== typed.trim()) answer(draft);
    else setDraft(undefined);
  };
  const done = q.aiDecides === true || saved.trim() !== "";
  return (
    <li className={done ? "plan-q plan-q--done" : "plan-q"}>
      <p className="plan-q-text">
        <Icon name={done ? "check" : "info"} size={13} />
        {q.text}
      </p>
      <div className="plan-q-answers">
        {options.map((o) => (
          <button
            key={o}
            className={`btn btn--sm plan-opt${!q.aiDecides && saved === o ? " is-picked" : ""}`}
            aria-pressed={!q.aiDecides && saved === o}
            onClick={() => answer(o)}
          >
            {o}
          </button>
        ))}
        <button
          className={`btn btn--sm plan-opt${q.aiDecides ? " is-picked" : ""}`}
          aria-pressed={q.aiDecides === true}
          title={t.questions.aiDecidesHint}
          onClick={() =>
            q.aiDecides
              ? onAction({ type: "answerQuestion", id: q.id })
              : onAction({ type: "answerQuestion", id: q.id, aiDecides: true })
          }
        >
          {t.questions.aiDecides}
        </button>
      </div>
      <div className="plan-q-own">
        <input
          className="input"
          placeholder={t.questions.other}
          value={value}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              save();
            }
          }}
        />
        <button className="btn btn--sm" disabled={draft === undefined || draft.trim() === typed.trim()} onClick={save}>
          {t.questions.save}
        </button>
      </div>
    </li>
  );
}

/** Harness and Test trình duyệt for this task: the AI's advice and the user's switch. */
export function OptionalSteps({ detail: d, onAction }: Props) {
  const proposals = d.proposals ?? [];
  const uses = d.uses ?? [];
  if (proposals.length === 0 && uses.length === 0) return null;
  return (
    <section className="plan-optional" aria-label={t.optional.title}>
      <h4>{t.optional.title}</h4>
      <p className="note">
        <Icon name="info" size={13} />
        {t.optional.note}
      </p>
      <ul>
        {SWITCHABLE.map((step) => {
          const p = proposals.find((x) => x.step === step);
          const state = d.steps.find((s) => s.kind === step)?.state;
          const open = state === undefined || state === "pending";
          const advice = p ? (p.recommend ? t.optional.recommend : t.optional.skip) : t.optional.noAdvice;
          return (
            <li key={step} className="plan-step">
              <label className="plan-step-switch">
                <input
                  type="checkbox"
                  checked={uses.includes(step)}
                  disabled={!open || d.status === "running" || d.parked === true}
                  onChange={(e) => onAction({ type: "setUses", step, on: e.target.checked })}
                />
                <strong>{stepLabel[step]}</strong>
                <span className={p?.recommend ? "plan-advice plan-advice--yes" : "plan-advice"}>{advice}</span>
                {!open && <span className="faint">· {t.optional.passed}</span>}
              </label>
              {p?.reason && <p className="plan-step-why">{p.reason}</p>}
              {p?.checks && (
                <div className="plan-step-checks">
                  <span className="faint">{t.optional.checks[step]}</span>
                  <p>{p.checks}</p>
                </div>
              )}
              {step === "browserTest" && (
                <DevServerBox
                  detail={d}
                  disabled={d.parked === true || (d.status === "running" && d.running?.step === "browserTest")}
                  onAction={onAction}
                />
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
