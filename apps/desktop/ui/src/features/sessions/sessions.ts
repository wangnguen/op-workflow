import { gateLabel, stepLabel, vi } from "../../i18n/vi";
import type {
  AgentKind,
  OrbitSnapshot,
  OutsideSession,
  SessionStatus,
  SessionView,
  StepKind,
  TaskSystem,
  WorkspaceSnapshot,
} from "../../ipc/types";
import { STEP_COUNT } from "../../shared/steps";
import type { Go } from "../orbit/primary";
import { repoName } from "../workspace/chatState";

/** One row of the sessions table: a session (the app's or one open outside it), or a task in
 *  progress that has no session yet. */
export interface SessionRow {
  id: string;
  kind: "app" | "outside" | "task";
  agent?: AgentKind;
  title: string;
  /** the task it belongs to */
  taskId?: string;
  /** where it is open, for a session outside the app (VS Code, terminal) */
  openIn?: string;
  status: SessionStatus;
  /** the state in words: "Chờ bạn", "Đang mở ở VS Code", … */
  stateText: string;
  /** what it does or did last, one line */
  activity?: string;
  updatedAt?: string;
  /** 0..100 of the session's context (or of the step's handoff threshold) */
  contextPct?: number;
  costUsd?: number;
  /** where "Tới phiên" (and the title) lead */
  to: Go;
  /** the session waits for an answer: "Tới phiên" lands on the question */
  asks: boolean;
  /** why Lưu trữ cannot run now; absent = it can */
  archiveBlocked?: string;
}

/** A repo and its rows. */
export interface ProjectRows {
  key: string;
  name: string;
  /** the folder, when a session gave it */
  path?: string;
  rows: SessionRow[];
}

const URGENT: Record<SessionStatus, number> = { waiting: 0, running: 1, idle: 2 };

/** "3/12 Viết code" */
function stepText(a: TaskSystem, kind: StepKind): string {
  const s = a.steps.find((x) => x.kind === kind);
  return s ? `${s.index}/${STEP_COUNT} ${stepLabel[kind]}` : stepLabel[kind];
}

/** Where the task stands by its steps: a gate needs you, a step that runs is running, a step
 *  ready to run is idle, nothing run by an AI = your turn. `undefined` = the steps say nothing
 *  more urgent than its sessions. */
function taskState(a: TaskSystem): { status: SessionStatus; activity: string } | undefined {
  const main = a.agents[0];
  const step = main ? stepText(a, main.step) : undefined;
  if (a.gate) return { status: "waiting", activity: step ? `${step} · ${gateLabel[a.gate.kind]}` : gateLabel[a.gate.kind] };
  if (main?.status === "running" || main?.status === "waiting") return { status: main.status, activity: step ?? "" };
  return undefined;
}

/** A task in progress with no Workspace session. */
function taskRow(a: TaskSystem): SessionRow {
  const main = a.agents[0];
  const step = main ? stepText(a, main.step) : undefined;
  const last = [...a.steps].reverse().find((s) => s.state === "done");
  const busy = taskState(a);
  const fallback =
    main && step
      ? { status: "idle" as const, activity: vi.flow.ready(step) }
      : { status: "waiting" as const, activity: last ? vi.sessions.yourTurn(stepLabel[last.kind]) : vi.sessions.notStarted };
  const { status, activity } = busy ?? fallback;
  return {
    id: `task:${a.taskId}`,
    kind: "task",
    agent: main?.kind,
    title: a.title,
    taskId: a.taskId,
    status,
    stateText: stateWords(status),
    activity,
    updatedAt: a.startedAt,
    contextPct: main && main.handoffAt > 0 ? (main.contextTokens / main.handoffAt) * 100 : undefined,
    costUsd: a.steps.reduce((sum, s) => sum + (s.costUsd ?? 0), 0) || undefined,
    to: { to: "session", taskId: a.taskId, ask: false },
    asks: false,
  };
}

const stateWords = (s: SessionStatus): string => vi.workspace.state[s] ?? s;

/** A session run by the app; a task's main session also carries the task's gate or running
 *  step. */
function appRow(v: SessionView, task: TaskSystem | undefined, main: boolean): SessionRow {
  const own: SessionStatus = v.state === "outside" ? "idle" : v.state;
  const fromTask = task && main ? taskState(task) : undefined;
  // the more urgent of the session itself and its task's steps
  const useTask = fromTask !== undefined && URGENT[fromTask.status] < URGENT[own];
  const status = useTask ? fromTask.status : own;
  const stateText =
    v.state === "outside" && v.outside ? vi.workspace.outsideAt(v.outside) : useTask ? stateWords(status) : (vi.workspace.state[v.state] ?? v.state);
  const blocked =
    v.state === "running"
      ? vi.sessions.archiveRunning
      : v.state === "outside"
        ? vi.sessions.archiveOutside
        : v.terminal || v.shell
          ? vi.sessions.archiveTerminal
          : undefined;
  return {
    id: `session:${v.id}`,
    kind: "app",
    agent: "claude",
    title: v.title || repoName(v.repo),
    taskId: v.taskId,
    status,
    stateText,
    activity: useTask ? fromTask.activity : v.state === "waiting" ? vi.sessions.sessionAsks : (v.error ?? v.activity),
    updatedAt: v.updatedAt,
    contextPct: v.contextPercent,
    costUsd: v.costUsd > 0 ? v.costUsd : undefined,
    to: { to: "chat", sessionId: v.id, ask: v.state === "waiting" },
    asks: v.state === "waiting",
    archiveBlocked: blocked,
  };
}

function outsideRow(o: OutsideSession): SessionRow {
  const status: SessionStatus = o.state === "outside" ? "idle" : o.state;
  return {
    id: `outside:${o.id}`,
    kind: "outside",
    agent: "claude",
    title: o.title,
    openIn: o.openIn,
    status,
    stateText: vi.workspace.state[o.state] ?? o.state,
    updatedAt: o.updatedAt,
    to: { to: "outside", id: o.id },
    asks: false,
  };
}

/** Every session (the app's, those open outside it) and every task in progress without a
 *  session, grouped by repo; most urgent first inside a repo (Chờ bạn, Đang chạy, Rảnh), then
 *  the newest. Repos keep the order of their name. */
export function sessionTable(s: OrbitSnapshot, ws?: WorkspaceSnapshot): ProjectRows[] {
  const groups = new Map<string, ProjectRows>();
  const add = (repo: string | undefined, row: SessionRow) => {
    const name = repo ? repoName(repo) : "";
    const key = name.toLowerCase();
    const g = groups.get(key) ?? { key, name, rows: [] };
    if (!g.path && repo && /[/\\]/.test(repo)) g.path = repo;
    g.rows.push(row);
    groups.set(key, g);
  };
  const tasks = [s.active, ...(s.others ?? [])].filter((a): a is TaskSystem => a !== undefined);
  const sessions = ws?.sessions ?? [];
  for (const a of tasks) {
    if (!sessions.some((v) => v.taskId === a.taskId)) add(a.repo, taskRow(a));
  }
  for (const v of sessions) {
    const task = v.taskId ? tasks.find((a) => a.taskId === v.taskId) : undefined;
    const mine = task ? sessions.filter((x) => x.taskId === v.taskId) : [];
    const main = (mine.find((x) => x.kind === "main") ?? mine[0])?.id === v.id;
    add(v.repo, appRow(v, task, main));
  }
  const outside = ws?.outside ?? [];
  for (const o of outside) add(o.cwd, outsideRow(o));
  // far stars the Workspace list does not have (another AI's sessions)
  for (const d of s.distant) {
    if (outside.some((o) => o.id === d.id)) continue;
    add(d.cwd ?? d.label, {
      id: `outside:${d.id}`,
      kind: "outside",
      agent: d.agent,
      title: d.title || d.label,
      openIn: d.openIn,
      status: d.status,
      stateText: stateWords(d.status),
      to: { to: "outside", id: d.id },
      asks: false,
    });
  }
  const time = (r: SessionRow) => (r.updatedAt ? Date.parse(r.updatedAt) || 0 : 0);
  const list = [...groups.values()];
  for (const g of list) g.rows.sort((a, b) => URGENT[a.status] - URGENT[b.status] || time(b) - time(a));
  return list.sort((a, b) => a.name.localeCompare(b.name));
}

/** How many rows of a repo need you and how many run, for its head line. */
export function tally(rows: SessionRow[]): { waiting: number; running: number } {
  return {
    waiting: rows.filter((r) => r.status === "waiting").length,
    running: rows.filter((r) => r.status === "running").length,
  };
}
