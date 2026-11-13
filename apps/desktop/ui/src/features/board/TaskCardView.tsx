import { agentLabel, stepLabel, vi } from "../../i18n/vi";
import type { TaskCard } from "../../ipc/types";
import { formatUsd } from "../../shared/format";
import { STEP_COUNT } from "../../shared/steps";

interface Props {
  card: TaskCard;
  locked: boolean;
  onStart: (card: TaskCard) => void;
}

const b = vi.board;

function primaryAction(card: TaskCard, locked: boolean, onStart: (c: TaskCard) => void) {
  if (card.column === "inbox") {
    return (
      <span title={locked ? b.locked : undefined}>
        <button className="btn btn--primary btn--sm" disabled={locked} onClick={() => onStart(card)}>
          {b.start}
        </button>
      </span>
    );
  }
  if (card.column === "awaitingPush") return <button className="btn btn--sm">{b.copy}</button>;
  if (card.step?.state === "waitingUser") {
    const label = card.step.kind === "analyze" ? b.reviewPlan : b.openTask;
    return <button className="btn btn--primary btn--sm">{label}</button>;
  }
  return (
    <>
      <button className="btn btn--sm">{b.openTask}</button>
      <button className="btn btn--ghost btn--sm">{b.pause}</button>
    </>
  );
}

export function TaskCardView({ card, locked, onStart }: Props) {
  const waiting = card.step?.state === "waitingUser";
  return (
    <article className={`card card--${card.column}${waiting ? " card--waiting" : ""}`}>
      <header className="card-head">
        <span className="card-id mono">{card.id}</span>
        <span className="card-status">{card.nxStatus}</span>
      </header>
      <h3 className="card-title">{card.title}</h3>
      <div className="card-meta">
        <span>{card.repo ?? card.project}</span>
        {card.branch && <span className="mono faint">{card.branch}</span>}
      </div>

      {card.step && (
        <div className="card-step">
          <span className={`step-dot step-dot--${card.step.state}`} />
          <span>{b.step(card.step.index, stepLabel[card.step.kind])}</span>
          <span className="card-steps" aria-hidden="true">
            {Array.from({ length: STEP_COUNT }, (_, i) => {
              const idx = i + 1;
              const cls =
                idx < (card.step?.index ?? 0) || card.step?.state === "done"
                  ? "is-done"
                  : idx === card.step?.index
                    ? `is-${card.step.state}`
                    : "";
              return <i key={idx} className={cls} />;
            })}
          </span>
        </div>
      )}

      {(card.agent || card.costUsd !== undefined) && (
        <div className="card-agent">
          {card.agent && <span className={`chip chip--${card.agent}`}>{agentLabel[card.agent]}</span>}
          {card.costUsd !== undefined && <span className="faint">{formatUsd(card.costUsd)}</span>}
        </div>
      )}

      {card.contextPct !== undefined && card.column === "active" && (
        <div className="card-ctx" title={`${b.context} ${Math.round(card.contextPct)}%`}>
          <div className={`bar${card.agent === "codex" ? " bar--codex" : ""}${card.contextPct >= 90 ? " bar--hot" : ""}`}>
            <span style={{ width: `${Math.min(100, card.contextPct)}%` }} />
          </div>
          <span className="faint">
            {b.context} {Math.round(card.contextPct)}%
          </span>
        </div>
      )}

      <footer className="card-foot">
        {card.newComments > 0 ? <span className="badge">{b.newComments(card.newComments)}</span> : <span />}
        <span className="card-actions">{primaryAction(card, locked, onStart)}</span>
      </footer>
    </article>
  );
}
