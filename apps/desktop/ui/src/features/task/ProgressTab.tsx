import { useEffect, useRef, useState } from "react";
import { agentLabel, stepLabel, vi } from "../../i18n/vi";
import type { DeniedCall, FlowAction, RuleOption, RunSummary, StepState, TaskDetail } from "../../ipc/types";
import { formatClock, formatDuration, formatHourMinute, formatTokens, formatUsd } from "../../shared/format";
import { useNow } from "../../shared/hooks";
import { modelText } from "../../shared/models";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { runMs, showsDenials } from "./runs";
import { TestsSection } from "./TestsSection";

const t = vi.flow.progress;

interface Props {
  detail: TaskDetail;
  busy?: string;
  onChat: (runId: number) => void;
  /** resolves to whether the rule went into config */
  onAllow: (rule: string) => Promise<boolean>;
  onResume: (runId: number) => void;
  onAction: (action: FlowAction) => void;
  /** opens a harness / library folder or a screenshot */
  onOpenPath: (path: string) => void;
}

const ICON: Record<RunSummary["state"], StepState> = {
  running: "running",
  done: "done",
  failed: "failed",
  waiting: "waitingUser",
};

/** Tiến trình: one row per run of a step (time, duration, cost, model, result in one line), the
 *  commands the CLI refused with "Cho phép lệnh này", and the detailed log folded below. */
export function ProgressTab({ detail: d, busy, onChat, onAllow, onResume, onAction, onOpenPath }: Props) {
  const running = d.runs.some((r) => r.state === "running");
  const now = useNow(running ? 1000 : 30_000);
  const [logOpen, setLogOpen] = useState(false);
  const intake = d.steps.find((s) => s.kind === "intake");
  const index = (step: RunSummary["step"]) => d.steps.find((s) => s.kind === step)?.index;
  return (
    <div className="task-pane">
      <TestsSection tests={d.tests ?? []} busy={busy} onAction={onAction} onOpenPath={onOpenPath} />
      <ol className="timeline" aria-label={t.label}>
        {intake?.startedAt && (
          <li className="tl-row tl-row--done">
            <span className="tl-icon">
              <StatusIcon state="done" />
            </span>
            <span className="mono faint tl-time">{formatHourMinute(intake.startedAt)}</span>
            <div className="tl-main">
              <div className="tl-head">
                <span className="tl-step">
                  {intake.index}. {stepLabel.intake}
                </span>
              </div>
              <p className="tl-note">{t.intakeNote}</p>
            </div>
          </li>
        )}
        {d.runs.map((r) => (
          <RunRow
            key={r.id}
            run={r}
            index={index(r.step)}
            now={now}
            busy={busy}
            taskRunning={running || d.status === "running"}
            onChat={onChat}
            onAllow={onAllow}
            onResume={onResume}
          />
        ))}
      </ol>
      {d.runs.length === 0 && <p className="dim">{t.empty}</p>}
      <section className="tl-log">
        <button className="btn btn--ghost btn--sm" aria-expanded={logOpen} onClick={() => setLogOpen((o) => !o)}>
          <Icon name="file" size={12} />
          {t.log(d.log.length)}
        </button>
        {logOpen && <LogList detail={d} />}
      </section>
    </div>
  );
}

interface RowProps {
  run: RunSummary;
  index?: number;
  now: number;
  busy?: string;
  taskRunning: boolean;
  onChat: (runId: number) => void;
  onAllow: (rule: string) => Promise<boolean>;
  onResume: (runId: number) => void;
}

function RunRow({ run: r, index, now, busy, taskRunning, onChat, onAllow, onResume }: RowProps) {
  const ms = runMs(r, now);
  const model = modelText(r.model, r.effort);
  const tokens = r.tokensIn !== undefined && r.tokensOut !== undefined ? t.tokens(formatTokens(r.tokensIn), formatTokens(r.tokensOut)) : "";
  return (
    <li className={`tl-row tl-row--${r.state}`}>
      <span className="tl-icon">
        <StatusIcon state={ICON[r.state]} />
      </span>
      <span className="mono faint tl-time" title={formatClock(r.startedAt)}>
        {formatHourMinute(r.startedAt)}
      </span>
      <div className="tl-main">
        <div className="tl-head">
          <span className="tl-step">
            {index ? `${index}. ` : ""}
            {stepLabel[r.step]}
            {r.round ? <span className="faint"> · {vi.orbit.round(r.round)}</span> : null}
          </span>
          <span className={`chip chip--${r.agent}`}>{agentLabel[r.agent]}</span>
          {model && <span className="faint tl-model">{model}</span>}
          <span className="tl-figures">
            {ms !== undefined && <span>{formatDuration(ms)}</span>}
            {r.costUsd ? <span>{formatUsd(r.costUsd)}</span> : null}
          </span>
        </div>
        <p className={`tl-note${r.state === "failed" ? " tl-note--failed" : ""}`}>
          {r.note ?? (r.state === "running" ? t.state.running : t.state[r.state])}
        </p>
        <div className="tl-actions">
          {tokens && <span className="faint tl-tokens">{tokens}</span>}
          {r.hasChat && (
            <button className="btn btn--ghost btn--xs" title={t.chatHint} onClick={() => onChat(r.id)}>
              <Icon name="file" size={11} />
              {t.chat}
            </button>
          )}
          {r.resume && (
            <button
              className="btn btn--ghost btn--xs"
              disabled={busy !== undefined || taskRunning}
              title={taskRunning ? t.resumeBusy : t.resumeHint(r.resume)}
              onClick={() => onResume(r.id)}
            >
              <Icon name="laptop" size={11} />
              {t.resume}
            </button>
          )}
        </div>
        {showsDenials(r) && (
          <div className="tl-denied" role="group" aria-label={t.deniedTitle}>
            <div className="tl-denied-head" title={t.deniedNote}>
              <Icon name="alert" size={12} />
              {t.denied(r.denials.length)}
            </div>
            {r.denials.map((call, i) => (
              <DeniedRow key={`${call.input}-${i}`} call={call} busy={busy} onAllow={onAllow} />
            ))}
            <p className="faint tl-denied-note">{t.deniedNote}</p>
          </div>
        )}
      </div>
    </li>
  );
}

function DeniedRow({ call, busy, onAllow }: { call: DeniedCall; busy?: string; onAllow: (rule: string) => Promise<boolean> }) {
  return (
    <div className="tl-call">
      <code className="mono tl-call-input">
        <span className="faint">{call.tool}: </span>
        {call.input}
      </code>
      {call.chained && <p className="faint tl-call-hint">{t.chained}</p>}
      {call.rules.length === 0 && <p className="faint tl-call-hint">{t.noRule}</p>}
      {call.rules.map((rule) => (
        <RuleRow key={rule.rule} option={rule} busy={busy} onAllow={onAllow} />
      ))}
    </div>
  );
}

/** One suggested rule: editable until it is in the allow list. A rule the user edited before
 *  allowing it stays editable, with "Đã cho phép" in place of the button, so nothing moves. */
function RuleRow({ option, busy, onAllow }: { option: RuleOption; busy?: string; onAllow: (rule: string) => Promise<boolean> }) {
  const [rule, setRule] = useState(option.rule);
  const [sent, setSent] = useState<string>();
  const send = () => {
    const r = rule.trim();
    if (!r || busy !== undefined) return;
    void onAllow(r).then((ok) => ok && setSent(r));
  };
  const allowedChip = (
    <span className="chip chip--done" title={t.allowedNote(sent ?? option.rule)}>
      <Icon name="check" size={11} />
      {t.allowed}
    </span>
  );
  if (option.allowed) {
    return (
      <div className="tl-rule">
        <span className="mono tl-rule-text">{option.rule}</span>
        {allowedChip}
      </div>
    );
  }
  return (
    <div className="tl-rule">
      <input
        className="input mono tl-rule-input"
        value={rule}
        aria-label={t.ruleLabel}
        spellCheck={false}
        onChange={(e) => setRule(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") send();
        }}
      />
      {sent !== undefined && sent === rule.trim() ? (
        allowedChip
      ) : (
        <button className="btn btn--sm" disabled={busy !== undefined || !rule.trim()} title={t.allowHint(rule.trim())} onClick={send}>
          {t.allow}
        </button>
      )}
    </div>
  );
}

/** The step log (what used to be the Log tab), newest at the bottom. */
function LogList({ detail: d }: { detail: TaskDetail }) {
  const box = useRef<HTMLOListElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [d.log.length]);
  if (d.log.length === 0) return <p className="dim">{vi.flow.logEmpty}</p>;
  return (
    <ol
      className="task-log task-log--folded"
      ref={box}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      }}
    >
      {d.log.map((l, i) => (
        <li key={`${l.at}-${i}`} className={`log-line log-line--${l.kind}`}>
          <span className="mono faint">{formatClock(l.at)}</span>
          <span className="log-step">{l.step ? stepLabel[l.step] : ""}</span>
          <span className={l.kind === "tool" ? "mono" : undefined}>{l.text}</span>
        </li>
      ))}
    </ol>
  );
}
