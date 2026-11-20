import { gateLabel, stepLabel, vi } from "../../i18n/vi";
import type {
  AgentKind,
  OrbitSnapshot,
  SessionStatus,
  SessionView,
  StepKind,
  TaskSystem,
  WorkspaceSnapshot,
} from "../../ipc/types";
import { STEP_COUNT } from "../../shared/steps";
import type { Go } from "../orbit/primary";
import { repoName } from "../workspace/chatState";

export interface SessionItem {
  id: string;
  /** the AI of the card; absent for a task in "Đang làm" whose steps run no AI right now */
  agent?: AgentKind;
  /** task the item belongs to (in-app tasks) */
  taskId?: string;
  title: string;
  detail: string;
  /** branch of the task, for in-app items */
  branch?: string;
  /** repo of the task or session */
  repo?: string;
  /** OpenProject status of the task, for in-app items */
  nxStatus?: string;
  nxStatusColor?: string;
  status: SessionStatus;
  /** 0..100 of the handoff threshold (or of the session's context), when known */
  contextPct?: number;
  inApp: boolean;
  /** where "Tới phiên" (and the title) lead */
  to: Go;
  /** the session waits for an answer: "Tới phiên" lands on the question */
  asks: boolean;
}

export type SessionGroups = Record<SessionStatus, SessionItem[]>;

/** What every task card shows about its task: name, OpenProject status, repo, branch. */
function taskInfo(a: TaskSystem) {
  return {
    taskId: a.taskId,
    title: `${a.taskId} · ${a.title}`,
    branch: a.branch || undefined,
    repo: a.repo || undefined,
    nxStatus: a.nxStatus || undefined,
    nxStatusColor: a.nxStatusColor,
    inApp: true,
  };
}

/** "3/12 Viết code" */
function stepText(a: TaskSystem, kind: StepKind): string {
  const s = a.steps.find((x) => x.kind === kind);
  return s ? `${s.index}/${STEP_COUNT} ${stepLabel[kind]}` : stepLabel[kind];
}

/** One card per task in progress: the most urgent of its steps and of its Workspace sessions.
 *  A gate or a session's question needs you; a step or a session that runs is running; a
 *  step that waits to be run is idle; nothing run by an AI = your turn. */
function taskCard(a: TaskSystem, sessions: SessionView[]): SessionItem {
  const main = a.agents[0];
  const asking = sessions.find((s) => s.state === "waiting");
  const talking = sessions.find((s) => s.state === "running");
  const step = main ? stepText(a, main.step) : undefined;
  const last = [...a.steps].reverse().find((s) => s.state === "done");
  let status: SessionStatus;
  let detail: string;
  if (a.gate) {
    status = "waiting";
    detail = step ? `${step} · ${gateLabel[a.gate.kind]}` : gateLabel[a.gate.kind];
  } else if (asking) {
    status = "waiting";
    detail = vi.sessions.sessionAsks;
  } else if (main?.status === "running" || main?.status === "waiting") {
    status = main.status;
    detail = step ?? "";
  } else if (talking) {
    status = "running";
    detail = vi.sessions.sessionRunning;
  } else if (main && step) {
    status = "idle";
    detail = vi.flow.ready(step);
  } else {
    status = "waiting";
    detail = last ? vi.sessions.yourTurn(stepLabel[last.kind]) : vi.sessions.notStarted;
  }
  const session = sessions.find((s) => s.kind === "main") ?? sessions[0];
  const ratio = main && main.handoffAt > 0 ? (main.contextTokens / main.handoffAt) * 100 : session?.contextPercent;
  return {
    ...taskInfo(a),
    id: `task:${a.taskId}`,
    agent: main?.kind,
    detail,
    status,
    contextPct: ratio,
    to: { to: "session", taskId: a.taskId, ask: asking !== undefined && !a.gate },
    asks: asking !== undefined && !a.gate,
  };
}

/** A Workspace session without a task. */
function freeCard(v: SessionView): SessionItem {
  const status: SessionStatus = v.state === "outside" ? "idle" : v.state;
  const state = v.state === "outside" && v.outside ? vi.workspace.outsideAt(v.outside) : vi.workspace.state[v.state];
  return {
    id: `session:${v.id}`,
    agent: "claude",
    title: v.title,
    detail: `${vi.sessions.free} · ${state ?? v.state}`,
    repo: repoName(v.repo),
    status,
    contextPct: v.contextPercent,
    inApp: true,
    to: { to: "chat", sessionId: v.id, ask: v.state === "waiting" },
    asks: v.state === "waiting",
  };
}

/** Group what runs (tasks in progress, Workspace sessions without a task, Claude sessions open
 *  outside the app) by status. */
export function groupSessions(s: OrbitSnapshot, ws?: WorkspaceSnapshot): SessionGroups {
  const groups: SessionGroups = { waiting: [], running: [], idle: [] };
  const sessions = ws?.sessions ?? [];
  for (const a of [s.active, ...(s.others ?? [])]) {
    if (!a) continue;
    const card = taskCard(
      a,
      sessions.filter((x) => x.taskId === a.taskId),
    );
    groups[card.status].push(card);
  }
  for (const v of sessions) {
    if (v.wpId !== undefined) continue;
    const card = freeCard(v);
    groups[card.status].push(card);
  }
  for (const d of s.distant) {
    groups[d.status].push({
      id: `outside:${d.id}`,
      agent: d.agent,
      title: d.title || d.label,
      detail: d.openIn ? `${vi.sessions.outside} · ${d.openIn}` : vi.sessions.outside,
      repo: d.title ? d.label : undefined,
      status: d.status,
      inApp: false,
      to: { to: "outside", id: d.id },
      asks: false,
    });
  }
  return groups;
}
