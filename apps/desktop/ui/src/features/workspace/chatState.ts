import type {
  ChatItem,
  SessionChat,
  SessionPatch,
  SessionView,
  StepKind,
  WorkspaceSnapshot,
  WorkspaceTask,
} from "../../ipc/types";

/** The chat pane's tabs that another screen can open. */
export type PaneView = "chat" | "changes" | "brief" | "comments";

/** Where to land in the chat pane, from another screen: a tab, the message a step started with,
 *  or the question the session waits on. `at` tells two clicks on the same thing apart. */
export interface Focus {
  view?: PaneView;
  step?: StepKind;
  ask?: boolean;
  at: number;
}

/** What the chat pane shows: a session, a task (or new session) that has no message yet, or a
 *  session opened outside the app (read-only). */
export type Selection =
  | { kind: "session"; id: string; focus?: Focus }
  | { kind: "task"; wpId: number; focus?: Focus }
  | { kind: "outside"; id: string };

/** Both point at the same row of the list (whatever they focus on). */
export function samePick(a: Selection | undefined, b: Selection): boolean {
  if (a?.kind === "task") return b.kind === "task" && a.wpId === b.wpId;
  if (a === undefined || b.kind === "task") return false;
  return a.kind === b.kind && a.id === b.id;
}

/** Where another screen sends you in Workspace (the map, a card, a task window). */
export interface WorkspaceTarget {
  sessionId?: string;
  /** a task: its main session (the newest), or the task itself before it has one */
  wpId?: number;
  /** a session opened outside the app */
  outsideId?: string;
  view?: PaneView;
  step?: StepKind;
  /** the question the session waits on */
  ask?: boolean;
}

/** The pick a target lands on (none when Workspace has nothing for it). A task with `ask` prefers
 *  its session that waits. `at` makes the same target land again when sent twice. */
export function pickFor(target: WorkspaceTarget, snapshot: WorkspaceSnapshot | undefined, at: number): Selection | undefined {
  if (target.outsideId !== undefined) return { kind: "outside", id: target.outsideId };
  const { view, step, ask } = target;
  const focus: Focus | undefined = view || step || ask ? { view, step, ask, at } : undefined;
  if (target.sessionId !== undefined) return { kind: "session", id: target.sessionId, focus };
  if (target.wpId === undefined) return undefined;
  const mine = snapshot?.sessions.filter((s) => s.wpId === target.wpId) ?? [];
  const s =
    (ask ? mine.find((x) => x.state === "waiting") : undefined) ?? mine.find((x) => x.kind === "main") ?? mine[0];
  if (s) return { kind: "session", id: s.id, focus };
  // a task Workspace does not list (finished, no session left): nothing to open there
  return snapshot?.tasks.some((t) => t.wpId === target.wpId) ? { kind: "task", wpId: target.wpId, focus } : undefined;
}

/** The item a step's focus lands on: the newest message labelled with that step (a step run
 *  again lands on its last run). */
export function stepItemIndex(items: ChatItem[], step: StepKind): number {
  for (let i = items.length - 1; i >= 0; i--) if (items[i]?.step === step) return i;
  return -1;
}

/** A pushed change applied to the chat. `refetch` = a change was missed (or arrived before the
 *  chat was read), read the whole chat again. */
export function applyPatch(chat: SessionChat | undefined, patch: SessionPatch): SessionChat | "refetch" {
  if (!chat) return "refetch";
  if (patch.seq <= chat.seq) return chat;
  if (patch.seq !== chat.seq + 1 || patch.from > chat.items.length) return "refetch";
  return {
    session: patch.session,
    items: chat.items.slice(0, patch.from).concat(patch.items),
    dropped: patch.dropped,
    partial: patch.partial,
    asks: patch.asks,
    seq: patch.seq,
  };
}

export interface TaskNode {
  wpId: number;
  taskId: string;
  title: string;
  /** in progress (the board's "Đang làm") */
  active: boolean;
  repo?: string;
  branch?: string;
  /** newest first */
  sessions: SessionView[];
}

export interface ProjectNode {
  project: string;
  tasks: TaskNode[];
}

export interface Tree {
  projects: ProjectNode[];
  /** sessions without a task, newest first */
  free: SessionView[];
}

/** Where tasks go whose task is no longer in progress (paused, done). */
export const OTHER_PROJECT = "";

/** Project → task → sessions, plus the sessions without a task. A task in progress shows even
 *  without a session; a session of a task no longer in progress still shows under its task. */
export function buildTree(snapshot: WorkspaceSnapshot): Tree {
  const projects = new Map<string, ProjectNode>();
  const nodes = new Map<number, TaskNode>();
  const node = (t: WorkspaceTask, active: boolean): TaskNode => {
    const n: TaskNode = { ...t, active, sessions: [] };
    nodes.set(t.wpId, n);
    const p = projects.get(t.project) ?? { project: t.project, tasks: [] };
    p.tasks.push(n);
    projects.set(t.project, p);
    return n;
  };
  for (const t of snapshot.tasks) node(t, true);
  const free: SessionView[] = [];
  for (const s of snapshot.sessions) {
    if (s.wpId === undefined) {
      free.push(s);
      continue;
    }
    const n =
      nodes.get(s.wpId) ??
      node({ wpId: s.wpId, taskId: s.taskId ?? `#${s.wpId}`, title: "", project: OTHER_PROJECT, repo: s.repo, branch: s.branch }, false);
    n.sessions.push(s);
  }
  // tasks no longer in progress go last
  const list = [...projects.values()].sort(
    (a, b) => Number(a.project === OTHER_PROJECT) - Number(b.project === OTHER_PROJECT),
  );
  return { projects: list, free };
}

/** The session a task opens on: its newest one. */
export const taskSession = (n: TaskNode): SessionView | undefined => n.sessions[0];

/** The most urgent state of a few sessions (what a task or repo row shows). */
export function urgentState(states: SessionView["state"][]): SessionView["state"] | undefined {
  const order: SessionView["state"][] = ["waiting", "running", "outside", "idle"];
  return order.find((st) => states.includes(st));
}

/** The state the tree shows for a task: the most urgent of its sessions. */
export const taskState = (n: TaskNode): SessionView["state"] | undefined => urgentState(n.sessions.map((s) => s.state));

/** Folder name of a path (`C:/x/acme-web` → `acme-web`). */
export const repoName = (path: string): string => path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? path;

/** Sessions of one project folder, under its name in the tree. */
export interface RepoGroup<T> {
  /** the folder, the same whatever the slashes or letter case */
  key: string;
  /** the folder as the first session gave it */
  repo: string;
  name: string;
  items: T[];
}

/** Items grouped by their project folder (same folder whatever the slashes or letter case), in
 *  the order the folders first appear, items in their own order. */
export function byRepo<T>(items: T[], repoOf: (item: T) => string): RepoGroup<T>[] {
  const groups = new Map<string, RepoGroup<T>>();
  for (const item of items) {
    const repo = repoOf(item);
    const key = repo.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    const g = groups.get(key) ?? { key, repo, name: repoName(repo) || repo, items: [] };
    g.items.push(item);
    groups.set(key, g);
  }
  return [...groups.values()];
}
