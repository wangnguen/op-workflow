import { vi } from "../../i18n/vi";
import type { FlowAction } from "../../ipc/types";

const t = vi.flow;

interface Props {
  disabled: boolean;
  onAction: (a: FlowAction) => void;
}

/** The brief or the plan changed after the plan was approved: analyse again, or keep the plan. */
export function PlanChangedChoices({ disabled, onAction }: Props) {
  return <div className="task-gate-choices">
    <div className="task-gate-choice">
      <button className="btn btn--sm" disabled={disabled} onClick={() => onAction({ type: "reopen", step: "analyze" })}>{t.planChanged.reanalyze}</button>
      <small className="faint">{t.planChanged.reanalyzeHint}</small>
    </div>
    <div className="task-gate-choice">
      <button className="btn btn--primary btn--sm" disabled={disabled} onClick={() => onAction({ type: "keepPlan" })}>{t.planChanged.keep}</button>
      <small className="faint">{t.planChanged.keepHint}</small>
    </div>
  </div>;
}

/** A Claude step reached its cost cap: go on in the same session with another cap, or start over. */
export function BudgetChoices({ disabled, onAction, canGoOn, budget }: Props & { canGoOn: boolean; budget?: number }) {
  return <div className="task-gate-choices">
    <div className="task-gate-choice">
      <button className="btn btn--sm" disabled={disabled} onClick={() => onAction({ type: "retry" })}>{t.retryFresh}</button>
      <small className="faint">{t.budget.retryFreshHint}</small>
    </div>
    <div className="task-gate-choice">
      <button className="btn btn--primary btn--sm" disabled={disabled || !canGoOn} onClick={() => onAction({ type: "goOn", note: "" })}>
        {t.budget.goOn(budget)}
      </button>
      <small className="faint">{canGoOn ? t.budget.goOnHint : t.budget.goOnGone}</small>
    </div>
  </div>;
}
