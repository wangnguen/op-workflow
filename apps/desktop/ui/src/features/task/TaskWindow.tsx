import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  agentLabel,
  findingStateLabel,
  gateLabel,
  severityLabel,
  stepHint,
  stepLabel,
  vi,
} from "../../i18n/vi";
import {
  POLL_MS,
  allowBash,
  decideFinding,
  errorMessage,
  flowAction,
  getTaskDetail,
  isMockMode,
  openLogin,
  openResume,
  openTaskFolder,
  openTaskLink,
  saveBrief,
} from "../../ipc/client";
import type { Decision, FindingView, FlowAction, TaskDetail } from "../../ipc/types";
import { copyText } from "../../shared/clipboard";
import { formatDuration, formatUsd } from "../../shared/format";
import { usePolling, useNow } from "../../shared/hooks";
import { ChatTab } from "./ChatTab";
import { CommentsTab } from "./CommentsTab";
import { ContinueDialog } from "./ContinueDialog";
import { CopyPush } from "./CopyPush";
import { DiffTab } from "./DiffTab";
import { FinishDialog } from "./FinishDialog";
import { Markdown } from "../../shared/Markdown";
import { modelText } from "../../shared/models";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { ProgressTab } from "./ProgressTab";
import { lastEnded } from "./runs";
import {
  defaultTab,
  findingCounts,
  findingPlace,
  isSettled,
  sortFindings,
  unpicked,
  unpickedSevere,
  type TaskTab,
} from "./taskTabs";
import "./task.css";

const t = vi.flow;

interface Props {
  wpId: number;
  /** tab to open with; default: what the task needs now */
  tab?: TaskTab;
  onClose: () => void;
  /** an action changed the task: reload the board and the map */
  onChanged: () => void;
}

/** Brief, plan, review findings, push summary and log of one task, with the step buttons. */
export function TaskWindow({ wpId, tab: initialTab, onClose, onChanged }: Props) {
  const fetcher = useCallback(() => getTaskDetail(wpId), [wpId]);
  const polled = usePolling(fetcher, POLL_MS);
  const [latest, setLatest] = useState<TaskDetail>();
  // The caller's tab holds until the task moves on (a gate opens, a step starts); from then
  // on the window shows what the task needs, unless the user picked a tab themselves.
  const [picked, setPicked] = useState<TaskTab | undefined>(initialTab);
  const userPicked = useRef(false);
  const lastAuto = useRef<TaskTab>(undefined);
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  /** run shown in the Chat tab (undefined = the running or latest one) */
  const [chatRunId, setChatRunId] = useState<number>();
  /** "Đã push xong" / "Làm tiếp" open over the window */
  const [dialog, setDialog] = useState<"finish" | "continue">();

  // whichever is newer: the poll, or the answer of the last button
  useEffect(() => {
    if (polled.data) setLatest(polled.data);
  }, [polled.data]);
  const d = latest;

  const auto = d ? defaultTab(d) : undefined;
  useEffect(() => {
    if (auto === undefined) return;
    if (lastAuto.current !== undefined && lastAuto.current !== auto && !userPicked.current) setPicked(undefined);
    lastAuto.current = auto;
  }, [auto]);
  const tab: TaskTab | undefined = picked ?? auto;
  const setTab = (next: TaskTab) => {
    userPicked.current = true;
    setPicked(next);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // a dialog over the window takes Escape itself
      if (e.key === "Escape" && !e.defaultPrevented) onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  /** Resolves to whether the call worked (its error is shown above the tabs). */
  const run = (label: string, call: () => Promise<TaskDetail | null>): Promise<boolean> => {
    setBusy(label);
    setError(undefined);
    return call()
      .then((next) => {
        if (next) setLatest(next);
        onChanged();
        polled.refresh();
        return true;
      })
      .catch((e: unknown) => {
        setError(errorMessage(e));
        return false;
      })
      .finally(() => setBusy(undefined));
  };
  const act = (action: FlowAction) => run(action.type, () => flowAction(wpId, action));
  // the Comment tab marked comments seen or rewrote the brief: counts and brief changed
  const { refresh } = polled;
  const countsChanged = useCallback(() => {
    refresh();
    onChanged();
  }, [refresh, onChanged]);
  const decide = (f: FindingView, decision: Decision, reason?: string) =>
    run(`decide:${f.fingerprint}`, () => decideFinding({ wpId, fingerprint: f.fingerprint, decision, reason }));
  /** "Chọn N lỗi critical / high": press Sửa on each of them, one after the other */
  const pickSevere = (list: FindingView[]) =>
    run("pickSevere", async () => {
      let next: TaskDetail | null = null;
      for (const f of list) next = await decideFinding({ wpId, fingerprint: f.fingerprint, decision: "fix" });
      return next;
    });
  const allow = (rule: string) => run("allow", () => allowBash(wpId, rule));
  const resume = (runId: number) => run("resume", () => openResume(wpId, runId));
  const showChat = (runId: number) => {
    setChatRunId(runId);
    setTab("chat");
  };

  const counts = d ? findingCounts(d.findings) : { open: 0, fix: 0 };
  const fresh = d?.newComments ?? 0;
  const tabs = [
    { value: "progress" as const, label: t.tabs.progress },
    { value: "chat" as const, label: t.tabs.chat },
    { value: "brief" as const, label: t.tabs.brief },
    { value: "comments" as const, label: t.tabs.comments, count: fresh > 0 ? fresh : undefined },
    { value: "plan" as const, label: t.tabs.plan },
    { value: "findings" as const, label: t.tabs.findings, count: counts.open > 0 ? counts.open : undefined },
    { value: "diff" as const, label: t.tabs.diff },
    { value: "push" as const, label: t.tabs.push },
  ];

  return (
    <div className="scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet task-sheet" role="dialog" aria-modal="true" aria-labelledby="task-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="layers" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="task-title">{d?.title ?? "…"}</h2>
            {d && (
              <p title={[d.taskId, d.project, d.branch, d.base].filter(Boolean).join(" · ")}>
                <button
                  type="button"
                  className="link-btn mono"
                  title={vi.flow.comments.openTaskHint}
                  onClick={() => openTaskLink(d.wpId).catch((e: unknown) => setError(errorMessage(e)))}
                >
                  {d.taskId}
                </button>{" "}
                · {d.project}
                {d.branch && (
                  <>
                    {" · "}
                    <span className="mono">{d.branch}</span>
                    {d.base && (
                      <span className="faint">
                        {" "}
                        {t.base} {d.base}
                      </span>
                    )}
                  </>
                )}
              </p>
            )}
          </div>
          {d && d.costUsd > 0 && (
            <span className="chip" title={t.cost}>
              {formatUsd(d.costUsd)}
            </span>
          )}
          {!isMockMode() && (
            <button
              className="btn btn--ghost btn--icon"
              title={t.folder}
              aria-label={t.folder}
              onClick={() => run("folder", () => openTaskFolder(wpId))}
            >
              <Icon name="folder" size={15} />
            </button>
          )}
          <button className="btn btn--ghost btn--icon" aria-label={t.close} title={t.close} onClick={onClose}>
            <Icon name="close" size={15} />
          </button>
        </header>

        {!d && <div className="task-loading dim">{polled.error ?? "…"}</div>}
        {d && (
          <>
            <StepBar detail={d} />
            <StatusStrip
              detail={d}
              tab={tab}
              busy={busy}
              onError={setError}
              onAction={act}
              onLogin={(p) => run("login", () => openLogin(p))}
              onShow={setTab}
              onFinish={() => setDialog("finish")}
              onContinue={() => setDialog("continue")}
              onFeedback={() => {
                setTab("plan");
                setFeedbackOpen(true);
              }}
            />
            {error && <p className="note note--error task-error">{error}</p>}
            <div className="task-tabs">
              <SegmentedControl
                options={tabs}
                value={tab ?? "progress"}
                onChange={setTab}
                label={t.tabsLabel}
              />
              {isMockMode() && <small className="faint">{t.demoNote}</small>}
            </div>
            <div className={`task-body${tab === "chat" ? " task-body--chat" : ""}`} key={tab}>
              {tab === "progress" && (
                <ProgressTab
                  detail={d}
                  busy={busy}
                  onChat={showChat}
                  onAllow={allow}
                  onResume={resume}
                />
              )}
              {tab === "chat" && (
                <ChatTab detail={d} runId={chatRunId} busy={busy} onPick={setChatRunId} onResume={resume} />
              )}
              {tab === "brief" && (
                <BriefTab
                  detail={d}
                  busy={busy}
                  onSave={(text) => run("brief", () => saveBrief(wpId, text))}
                />
              )}
              {tab === "plan" && (
                <PlanTab
                  detail={d}
                  busy={busy}
                  feedbackOpen={feedbackOpen}
                  setFeedbackOpen={setFeedbackOpen}
                  onAction={act}
                />
              )}
              {tab === "comments" && <CommentsTab detail={d} onChanged={countsChanged} />}
              {tab === "findings" && (
                <FindingsTab detail={d} busy={busy} onDecide={decide} onPickSevere={pickSevere} onAction={act} />
              )}
              {tab === "diff" && <DiffTab detail={d} />}
              {tab === "push" && <PushTab detail={d} busy={busy} onAction={act} />}
            </div>
          </>
        )}
      </div>
      {d && dialog === "finish" && (
        <FinishDialog
          wpId={wpId}
          taskId={d.taskId}
          branch={d.branch}
          atPush={d.status === "awaitingPush"}
          stepName={d.current ? stepLabel[d.current] : undefined}
          onClose={() => setDialog(undefined)}
          onFinished={(next) => {
            setLatest(next);
            onChanged();
            polled.refresh();
          }}
        />
      )}
      {d && dialog === "continue" && (
        <ContinueDialog
          wpId={wpId}
          taskId={d.taskId}
          detail={d}
          onClose={() => setDialog(undefined)}
          onDone={(next) => {
            setDialog(undefined);
            setLatest(next);
            onChanged();
            polled.refresh();
          }}
        />
      )}
    </div>
  );
}

// ---------- steps and status ----------

function StepBar({ detail: d }: { detail: TaskDetail }) {
  return (
    <ol className="task-steps" aria-label={vi.orbit.panel.steps}>
      {d.steps.map((s) => (
        <li
          key={s.kind}
          className={`task-step task-step--${s.state}${s.kind === d.current ? " is-current" : ""}`}
          title={[stepHint[s.kind], s.costUsd ? formatUsd(s.costUsd) : "", s.model ? `${t.model}: ${s.model}${s.effort ? ` · ${s.effort}` : ""}` : ""]
            .filter(Boolean)
            .join(" · ")}
        >
          <StatusIcon state={s.state} />
          <span>
            {s.index}. {stepLabel[s.kind]}
            {/* the round sits on the second line, so a new round never makes the name wrap */}
            <small className="task-step-model">
              {[s.round ? vi.orbit.round(s.round) : "", modelText(s.model, s.effort)].filter(Boolean).join(" · ") || "\u00a0"}
            </small>
          </span>
        </li>
      ))}
    </ol>
  );
}

interface StripProps {
  detail: TaskDetail;
  /** the tab on screen (the findings gate offers "Xem lỗi" from the other tabs) */
  tab?: TaskTab;
  busy?: string;
  onError: (message: string) => void;
  onAction: (a: FlowAction) => void;
  onLogin: (p: "claude" | "codex") => void;
  onShow: (tab: TaskTab) => void;
  onFeedback: () => void;
  /** "Đã push xong": the push check dialog */
  onFinish: () => void;
  /** "Làm tiếp" on a finished task */
  onContinue: () => void;
}

function StatusStrip({ detail: d, tab, busy, onError, onAction, onLogin, onShow, onFeedback, onFinish, onContinue }: StripProps) {
  const now = useNow(1000);
  const step = d.current ? stepLabel[d.current] : "";
  const disabled = busy !== undefined;
  let icon: ReactNode = <StatusIcon state="pending" size={15} />;
  let text: ReactNode = t.status[d.status];
  let actions: ReactNode = null;
  let tone = d.status as string;

  if (d.parked) {
    // back in the Inbox: the history can be read; steps wait for "Bắt đầu làm việc"
    tone = "parked";
    text = (
      <>
        <strong>{vi.carryOn.parkedStrip}</strong>
        <span className="faint">{vi.carryOn.parkedStripNote}</span>
      </>
    );
  } else if (d.status === "ready" && d.current) {
    const last = lastEnded(d.runs);
    const said =
      last?.note &&
      (last.state === "failed"
        ? t.progress.lastFailed(stepLabel[last.step], last.note)
        : t.progress.lastDone(stepLabel[last.step], last.note));
    text = (
      <>
        <strong title={stepHint[d.current]}>{t.ready(step)}</strong>
        <span className="faint">{said || stepHint[d.current]}</span>
      </>
    );
    actions = (
      <>
        <AutoToggle detail={d} disabled={disabled} onAction={onAction} />
        <button className="btn btn--primary btn--sm" disabled={disabled} title={t.runHint} onClick={() => onAction({ type: "run" })}>
          <Icon name="play" size={12} />
          {t.run(step)}
        </button>
      </>
    );
  } else if (d.status === "running" && d.running) {
    const r = d.running;
    const last = r.tools[r.tools.length - 1];
    const how = [modelText(r.model, r.effort), r.command].filter(Boolean).join(" · ");
    icon = <StatusIcon state="running" size={15} />;
    text = (
      <>
        <strong>
          <span className={`chip chip--${r.agent}`}>{agentLabel[r.agent]}</span> {t.running(stepLabel[r.step].toLowerCase())}
          {how && <span className="faint task-status-how"> · {how}</span>}
        </strong>
        <span className="faint task-status-tool" title={last?.name}>
          {formatDuration(now - Date.parse(r.startedAt))}
          {last && (
            <>
              {" · "}
              <span className="mono">{last.name}</span>
            </>
          )}
        </span>
      </>
    );
    actions = (
      <>
        <AutoToggle detail={d} disabled={false} onAction={onAction} />
        <button className="btn btn--sm" disabled={busy === "stop"} title={t.stopHint} onClick={() => onAction({ type: "stop" })}>
          <Icon name="pause" size={12} />
          {t.stop}
        </button>
      </>
    );
  } else if (d.status === "gate" && d.gate) {
    const kind = d.gate.kind;
    icon = <StatusIcon state={kind === "error" ? "failed" : "waitingUser"} size={15} />;
    tone = kind === "error" ? "error" : "gate";
    text = (
      <>
        <strong>
          {gateLabel[kind]}
          {step && <span className="faint"> · {step}</span>}
        </strong>
        <span>{d.gate.message}</span>
      </>
    );
    if (kind === "error") {
      actions = (
        <>
          {d.error?.provider && (
            <button className="btn btn--sm" disabled={disabled} onClick={() => onLogin(d.error?.provider ?? "claude")}>
              {t.login}
            </button>
          )}
          <button className="btn btn--primary btn--sm" disabled={disabled} onClick={() => onAction({ type: "retry" })}>
            <Icon name="refresh" size={12} />
            {t.retry}
          </button>
        </>
      );
    } else if (kind === "planReview") {
      actions = (
        <>
          <button className="btn btn--ghost btn--sm" disabled={disabled} title={t.rejectHint} onClick={onFeedback}>
            {t.reject}
          </button>
          <button className="btn btn--sm" disabled={disabled} title={t.selfCodeHint} onClick={() => onAction({ type: "selfCode" })}>
            {t.selfCode}
          </button>
          <button className="btn btn--primary btn--sm" disabled={disabled} title={t.approveHint} onClick={() => onAction({ type: "approvePlan" })}>
            <Icon name="check" size={12} />
            {t.approve}
          </button>
        </>
      );
    } else if (kind === "findings") {
      actions = (
        <>
          <AutoToggle detail={d} disabled={disabled} onAction={onAction} />
          <FindingsButtons detail={d} disabled={disabled} onAction={onAction} onShow={tab === "findings" ? undefined : onShow} />
        </>
      );
    } else if (kind === "afterFix") {
      const more = unpicked(d.findings);
      const again = d.gate.review === "finalReview";
      actions = (
        <>
          <AutoToggle detail={d} disabled={disabled} onAction={onAction} />
          {more > 0 && (
            <button
              className="btn btn--ghost btn--sm"
              disabled={disabled}
              title={t.pickMoreHint}
              onClick={() => onAction({ type: "afterFix", choice: "pickMore" })}
            >
              {t.pickMore(more)}
            </button>
          )}
          <button
            className="btn btn--sm"
            disabled={disabled}
            title={t.noCrossHint(again)}
            onClick={() => onAction({ type: "afterFix", choice: "next" })}
          >
            {t.noCross(stepLabel.finalReview, again)}
          </button>
          <button
            className="btn btn--primary btn--sm"
            disabled={disabled}
            title={t.crossAgainHint}
            onClick={() => onAction({ type: "afterFix", choice: "crossReview" })}
          >
            <Icon name="refresh" size={12} />
            {t.crossAgain(d.round)}
          </button>
        </>
      );
    } else if (kind === "selfCode") {
      actions = (
        <button className="btn btn--primary btn--sm" disabled={disabled} title={t.selfCodeDoneHint} onClick={() => onAction({ type: "selfCodeDone" })}>
          <Icon name="check" size={12} />
          {t.selfCodeDone}
        </button>
      );
    }
  } else if (d.status === "awaitingPush") {
    icon = <StatusIcon state="done" size={15} />;
    text = (
      <>
        <strong>{t.status.awaitingPush}</strong>
        <span className="faint">{t.pushNote}</span>
      </>
    );
    actions = (
      <>
        <CopyPush push={d.push} onError={onError} />
        <button className="btn btn--primary btn--sm" disabled={disabled} title={t.finishHint} onClick={onFinish}>
          <Icon name="check" size={12} />
          {t.finish}
        </button>
      </>
    );
  } else if (d.status === "done") {
    icon = <StatusIcon state="done" size={15} />;
    text = (
      <>
        <strong>{t.status.done}</strong>
        <span className="faint">{t.doneNote}</span>
      </>
    );
    actions = (
      <button className="btn btn--primary btn--sm" disabled={disabled} title={vi.board.continueHint} onClick={onContinue}>
        <Icon name="play" size={12} />
        {vi.board.continue}
      </button>
    );
  }

  return (
    <div className={`task-status task-status--${tone}`} role="status">
      <span className="task-status-icon">{icon}</span>
      <span className="task-status-text">{text}</span>
      <span className="task-status-actions">{actions}</span>
    </div>
  );
}

/** The findings gate's answers: go on without fixing, or fix what the user picked. */
function FindingsButtons({
  detail: d,
  disabled,
  onAction,
  onShow,
}: {
  detail: TaskDetail;
  disabled: boolean;
  onAction: (a: FlowAction) => void;
  onShow?: (tab: TaskTab) => void;
}) {
  const picked = findingCounts(d.findings).fix;
  const next = d.gate?.next ? stepLabel[d.gate.next] : "";
  return (
    <>
      {onShow && (
        <button className="btn btn--ghost btn--sm" onClick={() => onShow("findings")}>
          {t.showFindings}
        </button>
      )}
      <button
        className="btn btn--sm"
        disabled={disabled}
        title={t.skipHint}
        onClick={() => onAction({ type: "pickFindings", fix: false })}
      >
        {t.skipTo(next)}
      </button>
      <button
        className="btn btn--primary btn--sm"
        disabled={disabled || picked === 0}
        title={picked > 0 ? t.pickFixHint : t.pickNoneHint}
        onClick={() => onAction({ type: "pickFindings", fix: true })}
      >
        <Icon name="check" size={12} />
        {t.pickFix(picked)}
      </button>
    </>
  );
}

function AutoToggle({ detail: d, disabled, onAction }: { detail: TaskDetail; disabled: boolean; onAction: (a: FlowAction) => void }) {
  return (
    <label className="task-auto" title={t.autoHint}>
      <input
        type="checkbox"
        checked={d.autoAdvance}
        disabled={disabled}
        onChange={(e) => onAction({ type: "setAuto", on: e.target.checked })}
      />
      {t.auto}
    </label>
  );
}

// ---------- tabs ----------

function BriefTab({ detail: d, busy, onSave }: { detail: TaskDetail; busy?: string; onSave: (text: string) => void }) {
  const [draft, setDraft] = useState<string>();
  const editable = d.status !== "running";
  if (!d.brief && draft === undefined) return <p className="dim">{t.noBrief}</p>;
  return (
    <div className="task-pane">
      <p className="note">
        <Icon name="info" size={13} />
        {t.briefNote}
      </p>
      {draft === undefined ? (
        <>
          <pre className="task-text">{d.brief}</pre>
          <div className="task-pane-actions">
            <button className="btn btn--sm" disabled={!editable} onClick={() => setDraft(d.brief ?? "")}>
              {t.editBrief}
            </button>
          </div>
        </>
      ) : (
        <>
          <textarea className="input task-textarea" value={draft} onChange={(e) => setDraft(e.target.value)} spellCheck={false} />
          <div className="task-pane-actions">
            <button className="btn btn--sm" onClick={() => setDraft(undefined)}>
              {t.cancelEdit}
            </button>
            <button
              className="btn btn--primary btn--sm"
              disabled={busy !== undefined || !draft.trim()}
              onClick={() => {
                onSave(draft);
                setDraft(undefined);
              }}
            >
              {t.saveBrief}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

interface PlanProps {
  detail: TaskDetail;
  busy?: string;
  feedbackOpen: boolean;
  setFeedbackOpen: (open: boolean) => void;
  onAction: (a: FlowAction) => void;
}

function PlanTab({ detail: d, busy, feedbackOpen, setFeedbackOpen, onAction }: PlanProps) {
  const [feedback, setFeedback] = useState("");
  if (!d.plan) return <p className="dim">{t.noPlan}</p>;
  const atGate = d.gate?.kind === "planReview";
  return (
    <div className="task-pane">
      {atGate && (
        <p className="note note--warn">
          <Icon name="alert" size={13} />
          {t.planGate}
        </p>
      )}
      <Markdown text={d.plan} className="task-md" />
      {atGate && feedbackOpen && (
        <div className="task-feedback">
          <textarea
            className="input task-textarea task-textarea--short"
            placeholder={t.rejectPlaceholder}
            value={feedback}
            autoFocus
            onChange={(e) => setFeedback(e.target.value)}
          />
          <div className="task-pane-actions">
            <button className="btn btn--sm" onClick={() => setFeedbackOpen(false)}>
              {t.cancelEdit}
            </button>
            <button
              className="btn btn--primary btn--sm"
              disabled={busy !== undefined || !feedback.trim()}
              onClick={() => {
                onAction({ type: "rejectPlan", feedback });
                setFeedbackOpen(false);
                setFeedback("");
              }}
            >
              {t.rejectSend}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

interface FindingsProps {
  detail: TaskDetail;
  busy?: string;
  onDecide: (f: FindingView, decision: Decision, reason?: string) => void;
  /** press Sửa on all these findings */
  onPickSevere: (list: FindingView[]) => void;
  onAction: (a: FlowAction) => void;
}

/** "Codex · Review chéo · vòng 2", "/review-local · Review chốt": who reported it, where. */
function sourceText(f: FindingView): string {
  return [t.source[f.source] ?? f.source, f.step ? stepLabel[f.step] : "", f.round ? t.findingRound(f.round) : ""]
    .filter(Boolean)
    .join(" · ");
}

function ReviewNotes({ detail: d }: { detail: TaskDetail }) {
  const notes = d.reviewNotes ?? [];
  if (notes.length === 0) return null;
  return (
    <section className="task-notes" aria-label={t.reviewNotes}>
      <h4 title={t.reviewNotesHint}>{t.reviewNotes}</h4>
      {notes.map((n) => (
        <div key={n.step} className="task-note">
          <span className="faint">{stepLabel[n.step]}</span>
          <Markdown text={n.text} className="task-md" />
        </div>
      ))}
    </section>
  );
}

function FindingsTab({ detail: d, busy, onDecide, onPickSevere, onAction }: FindingsProps) {
  const [accepting, setAccepting] = useState<string>();
  const [reason, setReason] = useState("");
  if (d.findings.length === 0) {
    return (
      <div className="task-pane">
        <p className="dim">{t.noFindings}</p>
        <ReviewNotes detail={d} />
      </div>
    );
  }
  const atGate = d.gate?.kind === "findings";
  const counts = findingCounts(d.findings);
  const severe = unpickedSevere(d.findings);
  const locked = d.status === "running";
  return (
    <div className="task-pane">
      <div className="task-findings-head">
        <span className="dim">{t.findingsSummary(counts.open, counts.fix)}</span>
        {d.round > 0 && <span className="faint">{t.round(d.round)}</span>}
        {atGate && (
          <span className="task-findings-actions">
            {severe.length > 0 && (
              <button
                className="btn btn--ghost btn--sm"
                disabled={busy !== undefined}
                title={t.pickSevereHint}
                onClick={() => onPickSevere(severe)}
              >
                {t.pickSevere(severe.length)}
              </button>
            )}
            <FindingsButtons detail={d} disabled={busy !== undefined} onAction={onAction} />
          </span>
        )}
      </div>
      <ul className="task-findings">
        {sortFindings(d.findings).map((f) => {
          const settled = isSettled(f);
          const asked = atGate && f.state === "open" && f.chosen !== true;
          const picked = f.chosen === true && f.state === "open";
          return (
            <li key={f.fingerprint} className={`finding finding--${f.severity} finding--${f.state}${asked ? " is-asked" : ""}`}>
              <span className={`sev sev--${f.severity}`}>{severityLabel[f.severity]}</span>
              <div className="finding-main">
                <div className="finding-title">{f.title}</div>
                <div className="finding-meta">
                  <span className="mono">{findingPlace(f)}</span>
                  <span title={t.sourceHint}>{sourceText(f)}</span>
                  <span className={`finding-state finding-state--${f.state}`}>
                    {f.state === "fixing" && d.status !== "running" ? t.fixingWaiting : findingStateLabel[f.state]}
                  </span>
                  {picked && <span className="finding-state finding-state--chosen">{t.chosen}</span>}
                </div>
                {f.detail && <p className="finding-detail">{f.detail}</p>}
                {f.suggestion && (
                  <p className="finding-detail faint">
                    {t.suggestion}: {f.suggestion}
                  </p>
                )}
                {f.reason && <p className="finding-detail">“{f.reason}”</p>}
                {accepting === f.fingerprint && (
                  <div className="finding-reason">
                    <input
                      className="input"
                      autoFocus
                      placeholder={t.reasonPlaceholder}
                      value={reason}
                      onChange={(e) => setReason(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && reason.trim()) {
                          onDecide(f, "accept", reason);
                          setAccepting(undefined);
                        }
                      }}
                    />
                    <button className="btn btn--sm" onClick={() => setAccepting(undefined)}>
                      {t.cancelEdit}
                    </button>
                    <button
                      className="btn btn--primary btn--sm"
                      disabled={!reason.trim()}
                      onClick={() => {
                        onDecide(f, "accept", reason);
                        setAccepting(undefined);
                      }}
                    >
                      {t.reasonSave}
                    </button>
                  </div>
                )}
              </div>
              <div className="finding-actions">
                {(settled || f.chosen) && f.state !== "fixed" ? (
                  <button className="btn btn--ghost btn--sm" disabled={locked || busy !== undefined} title={t.decideHint.reset} onClick={() => onDecide(f, "reset")}>
                    {t.decide.reset}
                  </button>
                ) : null}
                {!settled && !f.chosen && (
                  <>
                    <button className="btn btn--sm" disabled={locked || busy !== undefined} title={t.decideHint.fix} onClick={() => onDecide(f, "fix")}>
                      {t.decide.fix}
                    </button>
                    <button
                      className="btn btn--sm"
                      disabled={locked || busy !== undefined}
                      title={t.decideHint.accept}
                      onClick={() => {
                        setReason("");
                        setAccepting(f.fingerprint);
                      }}
                    >
                      {t.decide.accept}
                    </button>
                    <button className="btn btn--ghost btn--sm" disabled={locked || busy !== undefined} title={t.decideHint.defer} onClick={() => onDecide(f, "defer")}>
                      {t.decide.defer}
                    </button>
                  </>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      <ReviewNotes detail={d} />
    </div>
  );
}

function PushTab({ detail: d, busy, onAction }: { detail: TaskDetail; busy?: string; onAction: (a: FlowAction) => void }) {
  const [issue, setIssue] = useState("");
  const [copied, setCopied] = useState<number>();
  const [copyError, setCopyError] = useState<string>();
  useEffect(() => {
    setIssue(d.push?.issue ? String(d.push.issue) : "");
  }, [d.push?.issue]);
  if (!d.push) return <p className="dim">{t.noPush}</p>;
  const copy = (text: string, i: number) => {
    setCopyError(undefined);
    copyText(text)
      .then(() => {
        setCopied(i);
        window.setTimeout(() => setCopied((c) => (c === i ? undefined : c)), 1500);
      })
      .catch(() => setCopyError(t.copyFailed));
  };
  const number = Number.parseInt(issue, 10);
  return (
    <div className="task-pane">
      <p className="note">
        <Icon name="info" size={13} />
        {t.pushTabNote}
      </p>
      {copyError && <p className="note note--error">{copyError}</p>}
      <div className="task-push-issue">
        <label>
          {t.issue}{" "}
          <input
            className="input task-issue"
            inputMode="numeric"
            value={issue}
            onChange={(e) => setIssue(e.target.value.replace(/\D/g, ""))}
          />
        </label>
        <button
          className="btn btn--sm"
          disabled={busy !== undefined || d.status !== "awaitingPush"}
          title={t.rerunPushHint}
          onClick={() => onAction({ type: "rerunPush", issue: Number.isFinite(number) ? number : undefined })}
        >
          <Icon name="refresh" size={12} />
          {t.rerunPush}
        </button>
      </div>
      {d.push.blocks.map((b, i) => (
        <section key={`${b.label}-${i}`} className="task-push-block">
          <header>
            <h4>{b.label}</h4>
            <button className="btn btn--sm" onClick={() => copy(b.text, i)}>
              <Icon name={copied === i ? "check" : "file"} size={12} />
              {copied === i ? t.copied : t.copy}
            </button>
          </header>
          <pre className="task-text">{b.text}</pre>
        </section>
      ))}
    </div>
  );
}
