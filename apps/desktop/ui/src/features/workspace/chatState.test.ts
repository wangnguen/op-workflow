import { describe, expect, it } from "vitest";
import type { SessionChat, SessionPatch, SessionView, WorkspaceSnapshot } from "../../ipc/types";
import { OTHER_PROJECT, applyPatch, buildTree, repoName, taskState } from "./chatState";

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
