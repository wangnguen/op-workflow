import type { ReactNode } from "react";
import {
  agentLabel,
  findingStateLabel,
  gateLabel,
  sessionStatusLabel,
  severityLabel,
  stepLabel,
  stepStateLabel,
  vi,
} from "../../i18n/vi";
import type { AgentView, OrbitSnapshot, StepView, TaskSystem } from "../../ipc/types";
import { formatClock, formatDuration, formatRelative, formatTokens, formatUsd } from "../../shared/format";
import { STEP_COUNT } from "../../shared/steps";
import type { Go, Primary } from "./primary";
import { findAgent, type Selection } from "./selection";
import { Icon } from "../../shared/ui/Icon";
import { PriorityChip } from "../../shared/ui/PriorityChip";
import { HideSideButton } from "../../shared/ui/SidePanel";
import { StatusChip } from "../../shared/ui/StatusChip";
import type { TaskTab } from "../task/taskTabs";

interface Props {
  snapshot: OrbitSnapshot;
  selection: Selection | null;
  onSelect: (s: Selection) => void;
  /** start an Inbox task (absent: the button explains to use the Task board) */
  onStartTask?: (taskId: string) => void;
  /** open the task window of a task in progress, on the tab that matters */
  onOpenTask?: (taskId: string, tab?: TaskTab) => void;
  /** open a task on OpenProject in the browser */
  onOpenLink?: (wpId: number) => void;
  /** the picked item's main button, on top (a double click on the map does the same) */
  primary?: Primary;
  onGo?: (go: Go) => void;
  now: number;
  /** hide the panel (it sits in a SidePanel) */
  onHide?: () => void;
}

const t = vi.orbit.panel;

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="dp-row">
      <span className="dp-key">{label}</span>
      <span className="dp-val">{children}</span>
    </div>
  );
}

/** Which task a step, session, finding or gate belongs to; click for the task's own panel. */
function TaskLine({ task, onSelect }: { task: TaskSystem; onSelect: (s: Selection) => void }) {
  return (
    <button className="dp-task" title={t.taskHint} onClick={() => onSelect({ kind: "star" })}>
      <span className="dp-task-top">
        <span className="mono dp-task-id">{task.taskId}</span>
        <StatusChip name={task.nxStatus} color={task.nxStatusColor} />
      </span>
      <span className="dp-task-title">{task.title}</span>
      <span className="mono dp-task-branch">{task.branch}</span>
    </button>
  );
}

function StepList({ steps, onSelect }: { steps: StepView[]; onSelect: (s: Selection) => void }) {
  return (
    <ol className="dp-steps">
      {steps.map((s) => (
        <li key={s.kind}>
          <button className={`dp-step dp-step--${s.state}`} onClick={() => onSelect({ kind: "step", step: s.kind })}>
            <span className="dp-dot" />
            <span>
              {s.index}. {stepLabel[s.kind]}
            </span>
            <span className="dp-step-state">
              {stepStateLabel[s.state]}
              {s.round && s.round > 1 ? ` · ${vi.orbit.round(s.round)}` : ""}
            </span>
          </button>
        </li>
      ))}
    </ol>
  );
}

interface AgentDetailProps {
  task: TaskSystem;
  agent: AgentView;
  now: number;
  onSelect: (s: Selection) => void;
  onOpenTask?: (taskId: string, tab?: TaskTab) => void;
  /** the main button already opens the task window's Chat tab */
  chatOnTop: boolean;
}

function AgentDetail({ task, agent, now, onSelect, onOpenTask, chatOnTop }: AgentDetailProps) {
  const ratio = agent.handoffAt > 0 ? agent.contextTokens / agent.handoffAt : 0;
  return (
    <>
      <TaskLine task={task} onSelect={onSelect} />
      <header className={`dp-head agent-${agent.kind}`}>
        <span className={`chip chip--${agent.kind}`}>{agentLabel[agent.kind]}</span>
        <span className={`dp-status dp-status--${agent.status}`}>{sessionStatusLabel[agent.status]}</span>
      </header>
      <Row label={t.step}>{stepLabel[agent.step]}</Row>
      <Row label={t.context}>
        {formatTokens(agent.contextTokens)} / {formatTokens(agent.handoffAt)} {t.handoff}
      </Row>
      <div className={`bar${agent.kind === "codex" ? " bar--codex" : ""}${ratio >= 0.9 ? " bar--hot" : ""}`}>
        <span style={{ width: `${Math.min(100, ratio * 100)}%` }} />
      </div>
      <Row label={t.compactions}>{agent.compactions}</Row>
      {onOpenTask && !chatOnTop && (
        <button className="btn btn--sm dp-open" title={t.openChatHint} onClick={() => onOpenTask(task.taskId, "chat")}>
          {t.openChat}
        </button>
      )}
      {agent.subagents.length > 0 && (
        <section>
          <h4>{t.subagents}</h4>
          <ul className="dp-list">
            {agent.subagents.map((s) => (
              <li key={s.name}>
                <span>{s.name}</span>
                <span className={s.status === "running" ? "dp-running" : "faint"}>
                  {s.status === "running" ? sessionStatusLabel.running : stepStateLabel.done}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
      {agent.children.length > 0 && (
        <section>
          <h4>{t.children}</h4>
          <ul className="dp-list">
            {agent.children.map((c) => (
              <li key={c.id}>
                <button className="dp-link" onClick={() => onSelect({ kind: "agent", id: c.id })}>
                  {agentLabel[c.kind]} · {stepLabel[c.step]}
                </button>
                <span className={c.status === "running" ? "dp-running" : "faint"}>{sessionStatusLabel[c.status]}</span>
              </li>
            ))}
          </ul>
        </section>
      )}
      {agent.recentTools.length > 0 && (
        <section>
          <h4>{t.tools}</h4>
          <ul className="dp-tools mono">
            {agent.recentTools.map((tool, i) => (
              <li key={`${tool.at}-${i}`} title={formatRelative(tool.at, now)}>
                <span className="faint">{formatClock(tool.at)}</span> {tool.name}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

export function DetailPanel({ snapshot, selection, onSelect, now, onStartTask, onOpenTask, onOpenLink, primary, onGo, onHide }: Props) {
  const a = snapshot.active;
  let body: ReactNode = <p className="dp-empty">{t.noneSelected}</p>;

  if (selection?.kind === "star" && a) {
    body = (
      <>
        <header className="dp-head">
          <span className="chip">{a.taskId}</span>
          <StatusChip name={a.nxStatus} color={a.nxStatusColor} />
        </header>
        <h3 className="dp-title">{a.title}</h3>
        <Row label={t.repo}>{a.repo}</Row>
        <Row label={t.branch}>
          <span className="mono">{a.branch}</span>
        </Row>
        <Row label={t.elapsed}>{formatDuration(now - Date.parse(a.startedAt))}</Row>
        {a.newComments > 0 && <Row label={t.newComments}>{a.newComments}</Row>}
        <Row label={t.findings}>{a.findings.length}</Row>
        {onOpenTask && (
          <button className="btn btn--sm dp-open" title={vi.flow.openHint} onClick={() => onOpenTask(a.taskId)}>
            {vi.flow.open}
          </button>
        )}
        <h4>{t.steps}</h4>
        <StepList steps={a.steps} onSelect={onSelect} />
      </>
    );
  } else if (selection?.kind === "step" && a) {
    const s = a.steps.find((x) => x.kind === selection.step);
    if (s) {
      body = (
        <>
          <TaskLine task={a} onSelect={onSelect} />
          <header className="dp-head">
            <span className="chip">
              {t.step} {s.index}/{STEP_COUNT}
            </span>
            <span className={`dp-status dp-status--${s.state}`}>{stepStateLabel[s.state]}</span>
          </header>
          <h3 className="dp-title">{stepLabel[s.kind]}</h3>
          <Row label={t.started}>{s.startedAt ? formatRelative(s.startedAt, now) : "—"}</Row>
          <Row label={t.ended}>{s.endedAt ? formatRelative(s.endedAt, now) : "—"}</Row>
          <Row label={t.cost}>{s.costUsd !== undefined ? formatUsd(s.costUsd) : "—"}</Row>
          {s.round !== undefined && <Row label={t.round}>{s.round}</Row>}
          <h4>{t.steps}</h4>
          <StepList steps={a.steps} onSelect={onSelect} />
        </>
      );
    }
  } else if (selection?.kind === "agent" && a) {
    const agent = findAgent(a.agents, selection.id);
    if (agent) {
      const chatOnTop = primary?.go.to === "task" && primary.go.tab === "chat";
      body = <AgentDetail task={a} agent={agent} now={now} onSelect={onSelect} onOpenTask={onOpenTask} chatOnTop={chatOnTop} />;
    }
  } else if (selection?.kind === "finding" && a) {
    const f = a.findings.find((x) => x.fingerprint === selection.fingerprint);
    if (f) {
      body = (
        <>
          <TaskLine task={a} onSelect={onSelect} />
          <header className="dp-head">
            <span className={`chip sev-chip sev-chip--${f.severity}`}>{severityLabel[f.severity]}</span>
            <span className="chip">{findingStateLabel[f.state]}</span>
          </header>
          <h3 className="dp-title">{f.title}</h3>
          <Row label={t.file}>
            <span className="mono">
              {f.file}
              {f.line !== undefined ? `:${f.line}` : ""}
            </span>
          </Row>
          <Row label={t.source}>{f.source}</Row>
          {f.detail && <p className="dim">{f.detail}</p>}
        </>
      );
    }
  } else if (selection?.kind === "gate" && a?.gate) {
    body = (
      <div className="dp-gate">
        <TaskLine task={a} onSelect={onSelect} />
        <header className="dp-head">
          <span className="chip chip--warn">{t.gate}</span>
          <span className="faint">{formatRelative(a.gate.since, now)}</span>
        </header>
        <h3 className="dp-title">{gateLabel[a.gate.kind]}</h3>
        <p>{a.gate.message}</p>
      </div>
    );
  } else if (selection?.kind === "nebula") {
    const item = snapshot.inbox.find((i) => i.taskId === selection.taskId);
    if (item) {
      body = (
        <>
          <header className="dp-head">
            <span className="chip">{t.inbox}</span>
            {item.nxStatus && <StatusChip name={item.nxStatus} color={item.nxStatusColor} />}
          </header>
          <h3 className="dp-title">{item.title}</h3>
          <Row label="Task">
            {onOpenLink ? (
              <button className="link-btn dp-nx-link" title={t.openOnNxHint} onClick={() => onOpenLink(item.wpId)}>
                {item.taskId}
                <Icon name="external" size={11} />
              </button>
            ) : (
              item.taskId
            )}
          </Row>
          <Row label={t.project}>{item.project}</Row>
          <Row label={t.priority}>{item.priority ? <PriorityChip priority={item.priority} /> : "—"}</Row>
          {onOpenLink && (
            <div className="dp-actions">
              <button className="btn btn--sm" title={t.openOnNxHint} onClick={() => onOpenLink(item.wpId)}>
                <Icon name="external" size={12} />
                {t.openOnNx}
              </button>
            </div>
          )}
        </>
      );
    }
  } else if (selection?.kind === "distant") {
    const d = snapshot.distant.find((x) => x.id === selection.id);
    if (d) {
      body = (
        <>
          <header className="dp-head">
            <span className={`chip chip--${d.agent}`}>{agentLabel[d.agent]}</span>
            <span className={`dp-status dp-status--${d.status}`}>{sessionStatusLabel[d.status]}</span>
          </header>
          <h3 className="dp-title">{d.title || d.label}</h3>
          <p className="dim">{t.distant}</p>
          {d.cwd && (
            <Row label={t.folder}>
              <span className="mono" title={d.cwd}>
                {d.label}
              </span>
            </Row>
          )}
          {d.openIn && <Row label={t.openIn}>{d.openIn}</Row>}
        </>
      );
    }
  }

  // the main button and the hide button share the top row; what the button does is under it
  const go = primary && onGo && selection ? primary : undefined;
  const pick = JSON.stringify(selection);
  return (
    <aside className="detail-panel">
      {(go || onHide) && (
        <div className={`dp-top${go ? " has-go" : ""}`}>
          <div className="dp-bar">
            {go && onGo && (
              <button
                key={pick}
                className="btn btn--primary btn--sm dp-go"
                disabled={go.go.to === "start" && !onStartTask}
                onClick={() => onGo(go.go)}
              >
                {go.label}
              </button>
            )}
            {onHide && <HideSideButton side="right" name={vi.side.detail} onHide={onHide} />}
          </div>
          {go && (
            <p className="faint dp-go-hint" key={pick}>
              {go.go.to === "start" && !onStartTask ? t.startFromBoard : go.hint}. {t.doubleClick}
            </p>
          )}
        </div>
      )}
      {body}
    </aside>
  );
}
