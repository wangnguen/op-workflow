import type { DraggableAttributes, DraggableSyntheticListeners } from "@dnd-kit/core";
import { agentLabel, stepHint, stepLabel, stepStateLabel, vi } from "../../i18n/vi";
import { errorMessage, getTaskDetail, openTaskLink } from "../../ipc/client";
import type { FlowAction, IssueStatus, Provider, PushCheck, TaskCard } from "../../ipc/types";
import { CopyPush } from "../task/CopyPush";
import type { TaskTab } from "../task/taskTabs";
import { formatHourMinute, formatRelative, formatUsd } from "../../shared/format";
import { STEP_COUNT } from "../../shared/steps";
import { Icon } from "../../shared/ui/Icon";
import { PriorityChip } from "../../shared/ui/PriorityChip";
import { StatusChip } from "../../shared/ui/StatusChip";
import { StatusIcon } from "../../shared/ui/StatusIcon";

/** What dnd-kit puts on a card that can be dragged (the card is its own handle). */
export interface CardDrag {
  ref: (el: HTMLElement | null) => void;
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners;
}

export interface TaskCardProps {
  card: TaskCard;
  onStart: (card: TaskCard) => void;
  onPause: (card: TaskCard) => void;
  /** absent in the browser preview (no brief on disk) */
  onBrief?: (card: TaskCard) => void;
  /** "Lấy issue": read the task's GitHub issues into its brief */
  onIssues?: (card: TaskCard) => void;
  readingIssues?: boolean;
  /** open the task window, optionally on one tab */
  onOpen: (card: TaskCard, tab?: TaskTab) => void;
  /** a workflow button (run, stop, retry, …) */
  onFlow: (card: TaskCard, action: FlowAction) => void;
  /** open `claude auth login` / `codex login` for a step that failed signed out */
  onLogin?: (provider: Provider) => void;
  /** a copy button failed (no push summary, clipboard refused) */
  onError?: (message: string) => void;
  /** "Đã push xong": the push check, then the task moves to Đã xong */
  onFinish: (card: TaskCard) => void;
  /** "Làm tiếp" on a finished task */
  onContinue: (card: TaskCard) => void;
  /** "N comment mới" on an Inbox card: show the comments without starting the task */
  onPeekComments?: (card: TaskCard) => void;
  /** the card can be dragged to another column (absent on the copy that follows the pointer) */
  drag?: CardDrag;
  /** this card is being dragged (its copy follows the pointer) */
  dragging?: boolean;
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

const f = vi.flow;

/** Intake done and nothing after it yet: the brief is what exists, issues can still be read. */
const beforeAnalyze = (card: TaskCard) =>
  (card.step?.kind === "intake" && card.step.state === "done") ||
  (card.step?.kind === "analyze" && card.step.state === "pending");

function primaryAction(props: TaskCardProps) {
  const { card, onStart, onPause, onBrief, onIssues, readingIssues, onOpen, onFlow, onLogin, onError, onFinish, onContinue } = props;
  if (card.column === "inbox") {
    return (
      <button className="btn btn--primary btn--sm" onClick={() => onStart(card)}>
        <Icon name="play" size={12} />
        {b.start}
      </button>
    );
  }
  const open = (tab?: TaskTab) => (
    <button className="btn btn--ghost btn--sm" title={f.openHint} onClick={() => onOpen(card, tab)}>
      {f.open}
    </button>
  );
  if (card.column === "done") {
    return (
      <>
        {open("progress")}
        <button className="btn btn--primary btn--sm" title={b.continueHint} onClick={() => onContinue(card)}>
          <Icon name="play" size={12} />
          {b.continue}
        </button>
      </>
    );
  }
  if (card.column === "awaitingPush") {
    return (
      <>
        {open("push")}
        <CopyPush load={() => getTaskDetail(card.wpId).then((d) => d.push)} onError={onError} />
        <button className="btn btn--primary btn--sm" title={f.finishHint} onClick={() => onFinish(card)}>
          <Icon name="check" size={12} />
          {f.finish}
        </button>
      </>
    );
  }
  const pause = (
    <button className="btn btn--ghost btn--sm" title={b.pauseHint} onClick={() => onPause(card)}>
      <Icon name="pause" size={12} />
      {b.pause}
    </button>
  );
  const step = card.step;
  if (step?.state === "running") {
    return (
      <>
        {open("progress")}
        <button className="btn btn--sm" title={f.stopHint} onClick={() => onFlow(card, { type: "stop" })}>
          <Icon name="pause" size={12} />
          {f.stop}
        </button>
      </>
    );
  }
  if (step?.state === "failed") {
    const login = card.login;
    return (
      <>
        {pause}
        {open("progress")}
        {login && onLogin && (
          <button className="btn btn--sm" title={vi.quota.loginHint(login === "codex" ? "Codex" : "Claude")} onClick={() => onLogin(login)}>
            {f.login}
          </button>
        )}
        <button className="btn btn--primary btn--sm" onClick={() => onFlow(card, { type: "retry" })}>
          <Icon name="refresh" size={12} />
          {f.retry}
        </button>
      </>
    );
  }
  if (step?.state === "waitingUser") {
    if (card.gate === "selfCode") {
      return (
        <>
          {pause}
          {open()}
          <button className="btn btn--primary btn--sm" title={f.selfCodeDoneHint} onClick={() => onFlow(card, { type: "selfCodeDone" })}>
            <Icon name="check" size={12} />
            {f.selfCodeDone}
          </button>
        </>
      );
    }
    const tab: TaskTab = card.gate === "mediumTriage" ? "findings" : "plan";
    const label = card.gate === "mediumTriage" ? f.triage : b.reviewPlan;
    return (
      <>
        {pause}
        <button className="btn btn--primary btn--sm" onClick={() => onOpen(card, tab)}>
          {label}
        </button>
      </>
    );
  }
  if (step?.state === "pending" && step.kind !== "analyze") {
    return (
      <>
        {pause}
        {open()}
        <button className="btn btn--primary btn--sm" title={f.runHint} onClick={() => onFlow(card, { type: "run" })}>
          <Icon name="play" size={12} />
          {f.run(stepLabel[step.kind])}
        </button>
      </>
    );
  }
  // Before analyze: read the issues into the brief first, then run the analysis.
  if (beforeAnalyze(card)) {
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
            {card.issues ? b.issuesAgain : b.issues}
          </button>
        )}
        {onBrief && (
          <button className="btn btn--ghost btn--sm btn--icon" title={b.brief} aria-label={b.brief} onClick={() => onBrief(card)}>
            <Icon name="file" size={13} />
          </button>
        )}
        {open("brief")}
        {step?.kind === "analyze" && (
          <button className="btn btn--primary btn--sm" title={f.runHint} onClick={() => onFlow(card, { type: "run" })}>
            <Icon name="play" size={12} />
            {f.run(stepLabel.analyze)}
          </button>
        )}
      </>
    );
  }
  return (
    <>
      {pause}
      {open()}
    </>
  );
}

/** How the last "Lấy issue" went: "Đã lấy 2 issue · 17:40", or the error in red. */
function IssueLine({ status }: { status: IssueStatus }) {
  const time = formatHourMinute(status.at);
  if (status.error) {
    return (
      <p className="card-issues card-issues--error" title={status.error}>
        <strong>{b.issuesFailedAt(time)}</strong> {status.error}
      </p>
    );
  }
  return <p className="card-issues">{b.issuesDoneAt(status.read, time)}</p>;
}

/** "Đã xong" card: when, where it stopped if before the push summary, and the push check. */
function DoneLines({ card }: { card: TaskCard }) {
  const step = card.step;
  const early = step && step.kind !== "pushSummary";
  const when = card.finishedAt ? formatRelative(card.finishedAt, Date.now()) : undefined;
  const status = card.nxStatus.toLowerCase();
  const nxOpen = card.nxOpen && !status.includes("resolved") && !status.includes("closed");
  return (
    <>
      <div className="card-line card-line--done" title={card.finishedAt}>
        <StatusIcon state="done" />
        <span className="card-line-text">
          {when ? b.finishedAt(when) : f.status.done}
          {early && <span className="faint"> · {b.stoppedAt(step.index, stepLabel[step.kind])}</span>}
        </span>
      </div>
      {card.pushCheck && <PushLine check={card.pushCheck} />}
      {nxOpen && (
        <p className="card-remind">
          <Icon name="alert" size={12} />
          {b.nxStillOpen(card.nxStatus)}
        </p>
      )}
    </>
  );
}

function PushLine({ check }: { check: PushCheck }) {
  const ok = check.pushed && !check.forced;
  return (
    <p className={`card-push${ok ? "" : " card-push--warn"}`} title={check.fetchError}>
      <Icon name={ok ? "cloud" : "alert"} size={12} />
      {ok ? vi.push.linePushed(check.head) : vi.push.lineForced(check.ahead, check.remote)}
    </p>
  );
}

export function TaskCardView(props: TaskCardProps) {
  const { card, drag, dragging } = props;
  const step = card.step;
  const waiting = step?.state === "waitingUser";
  const done = card.column === "done";
  return (
    <article
      ref={drag?.ref}
      {...drag?.attributes}
      {...drag?.listeners}
      className={`card card--${card.column}${waiting ? " card--waiting" : ""}${dragging ? " is-drag-source" : ""}`}
    >
      <span className="card-bar" aria-hidden="true" />
      <header className="card-top">
        <span className="card-mono" data-tone={projectTone(card.project)} title={card.project} aria-hidden="true">
          {monogram(card.project)}
        </span>
        <span className="card-heading">
          <h3 className="card-title">{card.title}</h3>
          <span className="card-sub">
            <button
              type="button"
              className="link-btn mono card-id"
              title={vi.flow.comments.openTaskHint}
              onClick={() => openTaskLink(card.wpId).catch((e: unknown) => props.onError?.(errorMessage(e)))}
            >
              {card.id}
            </button>
            <span className="card-sub-sep">·</span>
            <span className="card-project">{card.project}</span>
            {card.priority && <PriorityChip priority={card.priority} className="card-prio" />}
          </span>
        </span>
        <StatusChip
          name={card.nxStatus}
          color={card.nxStatusColor}
          className="card-nx"
          title={vi.start.statusNote(card.nxStatus)}
        />
      </header>

      {done && <DoneLines card={card} />}
      {step && !done && (
        <div className={`card-line card-line--${step.state}`} title={stepHint[step.kind]}>
          <StatusIcon state={step.state} />
          <span className="card-line-text">
            {b.step(step.index, stepLabel[step.kind])} ·{" "}
            {step.state === "pending" && card.column === "active" ? f.status.ready.toLowerCase() : stepStateLabel[step.state]}
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

      {card.flowNote ? (
        <p className={`card-note${step?.state === "failed" ? " card-note--error" : ""}`} title={card.flowNote}>
          {card.flowNote}
        </p>
      ) : (
        beforeAnalyze(card) && <p className="card-note">{b.nextStep}</p>
      )}
      {card.issues && card.column === "active" && beforeAnalyze(card) && <IssueLine status={card.issues} />}

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
        {card.newComments > 0 && card.column !== "inbox" ? (
          <button className="badge" title={vi.flow.comments.openHint} onClick={() => props.onOpen(card, "comments")}>
            {b.newComments(card.newComments)}
          </button>
        ) : card.newComments > 0 && props.onPeekComments ? (
          <button className="badge" title={b.peek.hint} onClick={() => props.onPeekComments?.(card)}>
            {b.newComments(card.newComments)}
          </button>
        ) : card.newComments > 0 ? (
          <span className="badge">{b.newComments(card.newComments)}</span>
        ) : (
          <span />
        )}
        <span className="card-actions">{primaryAction(props)}</span>
      </footer>
    </article>
  );
}
