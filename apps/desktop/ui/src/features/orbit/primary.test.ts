import { describe, expect, it } from "vitest";
import { orbitForFrame } from "../../ipc/mock";
import { primaryAction, runsInSession } from "./primary";

const NOW = Date.parse("2026-11-13T08:00:00Z");
const NONE = new Set<string>();

describe("primaryAction", () => {
  it("knows which steps run in the task's session", () => {
    expect(runsInSession("analyze")).toBe(true);
    expect(runsInSession("fixCross")).toBe(true);
    expect(runsInSession("crossReview")).toBe(false);
    expect(runsInSession("browserTest")).toBe(false);
    expect(runsInSession("implement", "claude-opus-5-5")).toBe(true);
    expect(runsInSession("implement", "gpt-5.5-codex")).toBe(false);
  });

  it("sends the star to the session and a planet to its step there or to the task's Chat tab", () => {
    const snap = orbitForFrame(10, NOW);
    const id = snap.active!.taskId;
    expect(primaryAction({ kind: "star" }, snap, NONE)?.go).toEqual({ to: "session", taskId: id });
    const done = snap.active!.steps.find((s) => s.state === "done" && s.kind === "analyze");
    if (done) expect(primaryAction({ kind: "step", step: "analyze" }, snap, NONE)?.go).toEqual({ to: "session", taskId: id, step: "analyze" });
    const cross = snap.active!.steps.find((s) => s.kind === "crossReview")!;
    const go = primaryAction({ kind: "step", step: "crossReview" }, snap, NONE)?.go;
    expect(go).toEqual(
      cross.state === "pending" || cross.state === "skipped" ? { to: "session", taskId: id } : { to: "task", taskId: id, tab: "chat" },
    );
  });

  it("sends a gate to the question when the session waits, else to the task window's tab", () => {
    const snap = orbitForFrame(2, NOW);
    const id = snap.active!.taskId;
    expect(primaryAction({ kind: "gate" }, snap, NONE)?.go.to).toBe("task");
    expect(primaryAction({ kind: "gate" }, snap, new Set([id]))?.go).toEqual({ to: "session", taskId: id, ask: true });
  });

  it("sends a far star to the outside session and a nebula to Bắt đầu", () => {
    const snap = orbitForFrame(0, NOW);
    expect(primaryAction({ kind: "distant", id: "ext-1" }, snap, NONE)?.go).toEqual({ to: "outside", id: "ext-1" });
    expect(primaryAction({ kind: "nebula", taskId: "WP-1" }, snap, NONE)?.go).toEqual({ to: "start", taskId: "WP-1" });
    expect(primaryAction(null, snap, NONE)).toBeUndefined();
  });
});
