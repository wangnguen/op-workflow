import { describe, expect, it } from "vitest";
import { orbitForFrame } from "../../ipc/mock";
import type { SessionView, WorkspaceSnapshot } from "../../ipc/types";
import { STEP_COUNT } from "../../shared/steps";
import { groupSessions } from "./sessions";

const NOW = Date.parse("2026-11-13T08:00:00Z");

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

const ws = (sessions: SessionView[]): WorkspaceSnapshot => ({ tasks: [], sessions });
const all = (g: ReturnType<typeof groupSessions>) => [...g.waiting, ...g.running, ...g.idle];

describe("groupSessions", () => {
  it("puts a task under Cần bạn while its gate is open", () => {
    const snap = orbitForFrame(2, NOW);
    const g = groupSessions(snap);
    expect(g.waiting.find((s) => s.id === `task:${snap.active!.taskId}`)).toMatchObject({
      to: { to: "session", taskId: snap.active!.taskId, ask: false },
      asks: false,
    });
  });

  it("gives each task in progress one card, with its name, status, repo, branch and step", () => {
    const snap = orbitForFrame(10, NOW);
    const tasks = [snap.active!, ...(snap.others ?? [])];
    const cards = all(groupSessions(snap)).filter((s) => s.taskId);
    expect(cards.map((c) => c.taskId).sort()).toEqual(tasks.map((t) => t.taskId).sort());
    for (const c of cards) {
      const task = tasks.find((t) => t.taskId === c.taskId)!;
      expect(c).toMatchObject({ title: `${task.taskId} · ${task.title}`, repo: task.repo, branch: task.branch, inApp: true });
      if (task.agents[0]) expect(c.detail).toMatch(new RegExp(`\\d/${STEP_COUNT} `));
    }
  });

  it("shows a task whose steps run no AI as your turn", () => {
    const snap = orbitForFrame(10, NOW);
    const idle = { ...snap.active!, taskId: "WP-9", branch: "f/WP-9", agents: [], gate: undefined };
    const item = groupSessions({ ...snap, others: [idle] }).waiting.find((s) => s.taskId === "WP-9");
    expect(item).toMatchObject({ id: "task:WP-9", branch: "f/WP-9", inApp: true });
    expect(item?.agent).toBeUndefined();
  });

  it("takes the most urgent state of the task's steps and its Workspace sessions", () => {
    const snap = orbitForFrame(10, NOW);
    const idle = { ...snap.active!, taskId: "WP-9", agents: [], gate: undefined };
    const orbit = { ...snap, active: idle, others: [] };
    const mine = (state: SessionView["state"]) => ws([view("s9", { kind: "main", wpId: 9, taskId: "WP-9", state })]);
    // the session asks something: Cần bạn, and "Tới phiên" lands on the question
    const asking = groupSessions(orbit, mine("waiting")).waiting.find((s) => s.taskId === "WP-9");
    expect(asking).toMatchObject({ asks: true, to: { to: "session", taskId: "WP-9", ask: true } });
    // Claude answers in the session: running, even though no step runs
    expect(groupSessions(orbit, mine("running")).running.some((s) => s.taskId === "WP-9")).toBe(true);
    // nothing runs anywhere: your turn
    expect(groupSessions(orbit, mine("idle")).waiting.some((s) => s.taskId === "WP-9")).toBe(true);
  });

  it("adds cards for sessions without a task and for sessions open outside the app", () => {
    const snap = orbitForFrame(10, NOW);
    const g = groupSessions(snap, ws([view("free-1", { state: "waiting" }), view("free-2", { state: "outside", outside: "VS Code" })]));
    expect(g.waiting.find((s) => s.id === "session:free-1")).toMatchObject({
      to: { to: "chat", sessionId: "free-1", ask: true },
      inApp: true,
    });
    expect(g.idle.find((s) => s.id === "session:free-2")?.detail).toContain("VS Code");
    const outside = all(g).filter((s) => !s.inApp);
    expect(outside).toHaveLength(snap.distant.length);
    for (const o of outside) expect(o.to.to).toBe("outside");
  });
});
