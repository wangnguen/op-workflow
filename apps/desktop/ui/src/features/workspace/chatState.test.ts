import { describe, expect, it } from "vitest";
import type { SessionChat, SessionPatch, SessionView, WorkspaceSnapshot } from "../../ipc/types";
import {
  OTHER_PROJECT,
  applyPatch,
  buildTree,
  pickFor,
  repoName,
  samePick,
  stepItemIndex,
  taskState,
} from "./chatState";

const view = (id: string, part: Partial<SessionView> = {}): SessionView => ({
  id,
  kind: "free",
  title: id,
  repo: "C:/code/web",
  state: "idle",
  live: false,
  costUsd: 0,
  started: true,
  createdAt: "2026-11-20T08:00:00Z",
  updatedAt: "2026-11-20T08:00:00Z",
  ...part,
});

const chat: SessionChat = {
  session: view("s1"),
  items: [
    { kind: "user", text: "hi" },
    { kind: "tool", tool: "Bash", text: "ls" },
  ],
  dropped: 0,
  seq: 5,
};

const patch = (part: Partial<SessionPatch>): SessionPatch => ({
  id: "s1",
  seq: 6,
  from: 1,
  items: [],
  dropped: 0,
  session: view("s1", { state: "running" }),
  ...part,
});

describe("applyPatch", () => {
  it("replaces the tail from the first changed item", () => {
    const next = applyPatch(
      chat,
      patch({ items: [{ kind: "tool", tool: "Bash", text: "ls", output: "a.ts" }, { kind: "text", text: "ok" }] }),
    );
    expect(next).not.toBe("refetch");
    if (next === "refetch") return;
    expect(next.items.map((i) => i.kind)).toEqual(["user", "tool", "text"]);
    expect(next.items[1]?.output).toBe("a.ts");
    expect(next.session.state).toBe("running");
    expect(next.seq).toBe(6);
  });

  it("ignores old patches and reads again after a missed one", () => {
    expect(applyPatch(chat, patch({ seq: 5 }))).toBe(chat);
    expect(applyPatch(chat, patch({ seq: 8 }))).toBe("refetch");
    expect(applyPatch(chat, patch({ from: 3 }))).toBe("refetch");
    expect(applyPatch(undefined, patch({}))).toBe("refetch");
  });

  it("keeps live text and asks from the patch", () => {
    const next = applyPatch(
      chat,
      patch({ from: 2, partial: "Đang", asks: [{ id: "r", kind: "permission", tool: "Bash", summary: "npm test" }] }),
    );
    if (next === "refetch") throw new Error("refetch");
    expect(next.items).toHaveLength(2);
    expect(next.partial).toBe("Đang");
    expect(next.asks?.[0]?.summary).toBe("npm test");
  });
});

describe("buildTree", () => {
  const snapshot: WorkspaceSnapshot = {
    tasks: [
      { wpId: 1, taskId: "WP-1", title: "A", project: "Web" },
      { wpId: 2, taskId: "WP-2", title: "B", project: "Web" },
      { wpId: 3, taskId: "WP-3", title: "C", project: "Api" },
    ],
    sessions: [
      view("new", { kind: "main", wpId: 1, state: "waiting" }),
      view("old", { kind: "main", wpId: 1, state: "idle" }),
      view("free1"),
      view("gone", { kind: "main", wpId: 9, taskId: "WP-9" }),
    ],
  };

  it("groups tasks by project and keeps sessions newest first", () => {
    const tree = buildTree(snapshot);
    expect(tree.projects.map((p) => p.project)).toEqual(["Web", "Api", OTHER_PROJECT]);
    const web = tree.projects[0]!;
    expect(web.tasks.map((t) => t.taskId)).toEqual(["WP-1", "WP-2"]);
    expect(web.tasks[0]!.sessions.map((s) => s.id)).toEqual(["new", "old"]);
    expect(web.tasks[1]!.sessions).toEqual([]);
    expect(taskState(web.tasks[0]!)).toBe("waiting");
    expect(taskState(web.tasks[1]!)).toBeUndefined();
    expect(tree.free.map((s) => s.id)).toEqual(["free1"]);
    const other = tree.projects[2]!.tasks[0]!;
    expect([other.taskId, other.active]).toEqual(["WP-9", false]);
  });

  it("names repos by their folder", () => {
    expect(repoName("C:\\Users\\me\\Desktop\\acme-web\\")).toBe("acme-web");
    expect(repoName("~/code/api")).toBe("api");
  });
});

describe("pickFor", () => {
  const snapshot: WorkspaceSnapshot = {
    tasks: [
      { wpId: 1, taskId: "WP-1", title: "A", project: "Web" },
      { wpId: 2, taskId: "WP-2", title: "B", project: "Web" },
    ],
    sessions: [
      view("asks", { kind: "free", wpId: 1, state: "waiting" }),
      view("main-new", { kind: "main", wpId: 1 }),
      view("main-old", { kind: "main", wpId: 1 }),
    ],
  };

  it("opens a task on its newest main session, focused when asked to", () => {
    expect(pickFor({ wpId: 1 }, snapshot, 5)).toEqual({ kind: "session", id: "main-new", focus: undefined });
    expect(pickFor({ wpId: 1, step: "analyze" }, snapshot, 5)).toEqual({
      kind: "session",
      id: "main-new",
      focus: { view: undefined, step: "analyze", ask: undefined, at: 5 },
    });
    // the question: the session that waits
    expect(pickFor({ wpId: 1, ask: true }, snapshot, 6)).toMatchObject({ kind: "session", id: "asks" });
  });

  it("opens a task without a session as a draft, and nothing for a task Workspace does not list", () => {
    expect(pickFor({ wpId: 2, view: "brief" }, snapshot, 7)).toEqual({
      kind: "task",
      wpId: 2,
      focus: { view: "brief", step: undefined, ask: undefined, at: 7 },
    });
    expect(pickFor({ wpId: 99 }, snapshot, 7)).toBeUndefined();
    expect(pickFor({}, snapshot, 7)).toBeUndefined();
  });

  it("opens sessions by id, outside ones included", () => {
    expect(pickFor({ sessionId: "x" }, undefined, 1)).toEqual({ kind: "session", id: "x", focus: undefined });
    expect(pickFor({ outsideId: "ext-1", wpId: 1 }, snapshot, 1)).toEqual({ kind: "outside", id: "ext-1" });
  });

  it("tells the same row apart from another whatever it focuses on", () => {
    const focus = { step: "fix" as const, at: 1 };
    expect(samePick({ kind: "session", id: "a", focus }, { kind: "session", id: "a" })).toBe(true);
    expect(samePick({ kind: "outside", id: "a" }, { kind: "session", id: "a" })).toBe(false);
    expect(samePick({ kind: "task", wpId: 1 }, { kind: "task", wpId: 1 })).toBe(true);
    expect(samePick(undefined, { kind: "task", wpId: 1 })).toBe(false);
  });

  it("lands a step on its latest run", () => {
    const items = [
      { kind: "prompt" as const, text: "a", step: "analyze" as const },
      { kind: "text" as const, text: "b" },
      { kind: "prompt" as const, text: "c", step: "fix" as const },
      { kind: "prompt" as const, text: "d", step: "fix" as const },
    ];
    expect(stepItemIndex(items, "fix")).toBe(3);
    expect(stepItemIndex(items, "analyze")).toBe(0);
    expect(stepItemIndex(items, "pushSummary")).toBe(-1);
  });
});
