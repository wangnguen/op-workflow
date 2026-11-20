import type { SessionChat, SessionPatch, SessionView, WorkspaceSnapshot, WorkspaceTask } from "../../ipc/types";

/** What the chat pane shows: a session, or a task (or new session) that has no message yet. */
export type Selection = { kind: "session"; id: string } | { kind: "task"; wpId: number };

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

/** The state the tree shows for a task: the most urgent of its sessions. */
export function taskState(n: TaskNode): SessionView["state"] | undefined {
  const order: SessionView["state"][] = ["waiting", "running", "outside", "idle"];
  return order.find((st) => n.sessions.some((s) => s.state === st));
}

/** Folder name of a path (`C:/x/acme-web` → `acme-web`). */
export const repoName = (path: string): string => path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? path;
