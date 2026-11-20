// Plan tab extras: the optional steps (Harness, Test trình duyệt), switched per task. The
// analysis may advise on them; the advice is only a hint and may be missing.
import type { FlowAction, StepKind, TaskDetail } from "../../ipc/types";
import { stepLabel, vi } from "../../i18n/vi";
import { Icon } from "../../shared/ui/Icon";
import { DevServerBox } from "./DevServerBox";

const t = vi.flow;
const SWITCHABLE: StepKind[] = ["harness", "browserTest"];

interface Props {
  detail: TaskDetail;
  onAction: (a: FlowAction) => void;
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
