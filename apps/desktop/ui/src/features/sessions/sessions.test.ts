import { describe, expect, it } from "vitest";
import { orbitForFrame } from "../../ipc/mock";
import { STEP_COUNT } from "../../shared/steps";
import { groupSessions } from "./sessions";

const NOW = Date.parse("2026-11-13T08:00:00Z");

describe("groupSessions", () => {
  it("puts the main agent under Cần bạn while a gate is open", () => {
    const g = groupSessions(orbitForFrame(2, NOW));
    expect(g.waiting.some((s) => s.id === "claude-main")).toBe(true);
  });

  it("includes child agents and outside sessions", () => {
    const g = groupSessions(orbitForFrame(10, NOW));
    const all = [...g.waiting, ...g.running, ...g.idle];
    expect(all.some((s) => s.agent === "codex" && s.inApp)).toBe(true);
    expect(all.filter((s) => !s.inApp)).toHaveLength(3);
  });

  it("shows a task in progress without an AI session as your turn, with its branch", () => {
    const snap = orbitForFrame(10, NOW);
    const active = snap.active!;
    const idle = { ...active, taskId: "WP-9", branch: "f/WP-9", agents: [], gate: undefined };
    const g = groupSessions({ ...snap, others: [idle] });
    const item = g.waiting.find((s) => s.taskId === "WP-9");
    expect(item).toMatchObject({ id: "task:WP-9", branch: "f/WP-9", inApp: true });
    expect(item?.agent).toBeUndefined();
    // sessions of a task with agents point back at their task
    expect(g.running.concat(g.idle, g.waiting).some((s) => s.agent && s.taskId === active.taskId)).toBe(true);
  });

  it("gives every AI session of a task the task's name, status, repo, branch and step", () => {
    const snap = orbitForFrame(10, NOW);
    const tasks = [snap.active!, ...(snap.others ?? [])];
    const g = groupSessions(snap);
    const sessions = [...g.waiting, ...g.running, ...g.idle].filter((s) => s.inApp && s.agent);
    expect(sessions.some((s) => s.taskId === snap.active!.taskId)).toBe(true);
    for (const s of sessions) {
      const task = tasks.find((t) => t.taskId === s.taskId)!;
      expect(s).toMatchObject({
        title: `${task.taskId} · ${task.title}`,
        repo: task.repo,
        branch: task.branch,
        nxStatus: task.nxStatus,
      });
      expect(s.detail).toMatch(new RegExp(`\\d/${STEP_COUNT} `));
    }
  });
});
