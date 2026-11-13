import { agentLabel, stepHint, stepLabel, stepStateLabel, vi } from "../../i18n/vi";
import type { TaskCard } from "../../ipc/types";
import { formatUsd } from "../../shared/format";
import { STEP_COUNT } from "../../shared/steps";
import { Icon } from "../../shared/ui/Icon";
import { StatusChip } from "../../shared/ui/StatusChip";
import { StatusIcon } from "../../shared/ui/StatusIcon";

interface Props {
  card: TaskCard;
  onStart: (card: TaskCard) => void;
  onPause: (card: TaskCard) => void;
  /** absent in the browser preview (no brief on disk) */
  onBrief?: (card: TaskCard) => void;
  /** "Lấy issue": read the task's GitHub issues into its brief */
  onIssues?: (card: TaskCard) => void;
  readingIssues?: boolean;
}

const b = vi.board;
const TONES = 6;

/** Same project, same tile colour on every card. */
function projectTone(project: string): number {
  let h = 0;
  for (const ch of project) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return h % TONES;
}

const monogram = (project: string) => (project.trim()[0] ?? "?").toUpperCase();

const intakeDone = (card: TaskCard) => card.step?.kind === "intake" && card.step.state === "done";

function primaryAction({ card, onStart, onPause, onBrief, onIssues, readingIssues }: Props) {
  if (card.column === "inbox") {
    return (
      <button className="btn btn--primary btn--sm" onClick={() => onStart(card)}>
        <Icon name="play" size={12} />
        {b.start}
      </button>
    );
  }
  if (card.column === "awaitingPush") return <button className="btn btn--sm">{b.copy}</button>;
  const pause = (
    <button className="btn btn--ghost btn--sm" title={b.pauseHint} onClick={() => onPause(card)}>
      <Icon name="pause" size={12} />
      {b.pause}
    </button>
  );
  // Intake done; the AI steps after it are not in the app yet, so the brief is what exists.
  if (intakeDone(card)) {
    return (
      <>
        {pause}
        {onIssues && (
          <button
            className="btn btn--ghost btn--sm"
            title={b.issuesHint}
            disabled={readingIssues}
            onClick={() => onIssues(card)}
          >
            <Icon name="refresh" size={12} className={readingIssues ? "is-spinning" : undefined} />
            {b.issues}
          </button>
        )}
        {onBrief && (
          <button className="btn btn--sm" title={b.briefHint} onClick={() => onBrief(card)}>
            <Icon name="file" size={13} />
            {b.brief}
          </button>
        )}
      </>
    );
  }
  if (card.step?.state === "waitingUser") {
    const label = card.step.kind === "analyze" ? b.reviewPlan : b.openTask;
    return <button className="btn btn--primary btn--sm">{label}</button>;
  }
  return (
    <>
      {pause}
      <button className="btn btn--sm">{b.openTask}</button>
    </>
  );
}

export function TaskCardView(props: Props) {
  const { card } = props;
  const step = card.step;
  const waiting = step?.state === "waitingUser";
  return (
    <article className={`card card--${card.column}${waiting ? " card--waiting" : ""}`}>
      <span className="card-bar" aria-hidden="true" />
      <header className="card-top">
        <span className="card-mono" data-tone={projectTone(card.project)} title={card.project} aria-hidden="true">
          {monogram(card.project)}
        </span>
        <span className="card-heading">
          <h3 className="card-title">{card.title}</h3>
          <span className="card-sub">
            <span className="mono">{card.id}</span>
            <span className="card-sub-sep">·</span>
            <span className="card-project">{card.project}</span>
          </span>
        </span>
        <StatusChip
          name={card.nxStatus}
          color={card.nxStatusColor}
          className="card-nx"
          title={vi.start.statusNote(card.nxStatus)}
        />
      </header>

      {step && (
        <div className={`card-line card-line--${step.state}`} title={stepHint[step.kind]}>
          <StatusIcon state={step.state} />
          <span className="card-line-text">
            {b.step(step.index, stepLabel[step.kind])} · {stepStateLabel[step.state]}
          </span>
          <span className="card-steps" aria-hidden="true">
            {Array.from({ length: STEP_COUNT }, (_, i) => {
              const idx = i + 1;
              const cls =
                idx < step.index || (idx === step.index && step.state === "done")
                  ? "is-done"
                  : idx === step.index
                    ? `is-${step.state}`
                    : "";
              return <i key={idx} className={cls} />;
            })}
          </span>
        </div>
      )}

      {card.branch && (
        <div className="card-branch" title={card.repo ? `${card.branch} · ${b.branchIn(card.repo)}` : card.branch}>
          <Icon name="branch" size={13} />
          <span className="mono card-branch-name">{card.branch}</span>
          {card.repo && <span className="card-repo">{card.repo}</span>}
        </div>
      )}

      {intakeDone(card) && <p className="card-note">{b.nextStep}</p>}

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
        <span className="card-actions">{primaryAction(props)}</span>
      </footer>
    </article>
  );
}
