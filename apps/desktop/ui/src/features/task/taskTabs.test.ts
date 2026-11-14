import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MOCK_STEP_MS, mockDecideFinding, mockFlowAction, mockFlowCard, mockTaskDetail, resetMockFlows } from "../../ipc/mockFlow";
import type { FindingView, TaskDetail } from "../../ipc/types";
import { parseBlocks } from "../../shared/Markdown";
import { defaultTab, findingCounts, gateTab, sortFindings, undecidedMediums } from "./taskTabs";

const finding = (over: Partial<FindingView>): FindingView => ({
  fingerprint: "x",
  severity: "low",
  title: "t",
  file: "a.ts",
  source: "codex",
  state: "open",
  ...over,
});

describe("task window helpers", () => {
  it("opens the tab the task needs", () => {
    const base = mockTaskDetail(1);
    const at = (over: Partial<TaskDetail>) => defaultTab({ ...base, ...over });
    expect(at({})).toBe("brief");
    expect(at({ status: "gate", gate: { kind: "planReview", since: "", message: "" } })).toBe("plan");
    expect(at({ status: "gate", gate: { kind: "mediumTriage", since: "", message: "" } })).toBe("findings");
    expect(at({ status: "gate", gate: { kind: "error", since: "", message: "" } })).toBe("log");
    expect(at({ status: "running" })).toBe("log");
    expect(at({ status: "awaitingPush" })).toBe("push");
    expect(gateTab("mediumTriage")).toBe("findings");
  });

  it("sorts open findings first, most severe first, and counts what the fix takes", () => {
    const list = [
      finding({ fingerprint: "a", severity: "low" }),
      finding({ fingerprint: "b", severity: "high", state: "fixed" }),
      finding({ fingerprint: "c", severity: "medium" }),
      finding({ fingerprint: "d", severity: "critical" }),
      finding({ fingerprint: "e", severity: "low", chosen: true }),
    ];
    expect(sortFindings(list).map((f) => f.fingerprint)).toEqual(["d", "c", "a", "e", "b"]);
    expect(findingCounts(list)).toEqual({ open: 4, fix: 3 });
    // while the user triages, mediums are theirs to decide, not the fix's
    expect(findingCounts(list, true)).toEqual({ open: 4, fix: 2 });
    expect(undecidedMediums(list)).toBe(1);
  });

  it("reads the markdown of a plan", () => {
    const blocks = parseBlocks("## Nguyên nhân\n`a.ts:1` lỗi\n\n- một\n- hai\n  tiếp\n1. x\n```\ncode\n```");
    expect(blocks.map((b) => b.kind)).toEqual(["h", "p", "ul", "ol", "code"]);
    expect(blocks[2]).toEqual({ kind: "ul", items: ["một", "hai tiếp"] });
  });
});

describe("mock workflow (browser preview)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(Date.parse("2026-11-14T08:00:00Z"));
    resetMockFlows();
  });
  afterEach(() => vi.useRealTimers());

  const step = (wpId: number) => vi.advanceTimersByTime(MOCK_STEP_MS + 10) && mockTaskDetail(wpId);

  it("goes through every step and every gate like the engine", () => {
    expect(mockTaskDetail(7)).toMatchObject({ status: "ready", current: "analyze" });
    mockFlowAction(7, { type: "run" });
    expect(mockFlowCard(7)?.step?.state).toBe("running");
    expect(step(7)).toMatchObject({ status: "gate", gate: { kind: "planReview" } });
    expect(() => mockFlowAction(7, { type: "rejectPlan", feedback: " " })).toThrow();
    mockFlowAction(7, { type: "approvePlan" });
    expect(step(7)).toMatchObject({ status: "ready", current: "reviewLocal" });
    mockFlowAction(7, { type: "run" });
    expect(step(7)).toMatchObject({ current: "fix" });
    mockFlowAction(7, { type: "run" });
    expect(mockTaskDetail(7).findings.find((f) => f.severity === "high")?.state).toBe("fixing");
    step(7);
    mockFlowAction(7, { type: "run" });
    const cross = step(7);
    expect(cross.current).toBe("finalReview");
    expect(cross.findings.find((f) => f.severity === "high")?.state).toBe("fixed");
    mockFlowAction(7, { type: "run" });
    const triage = step(7);
    expect(triage.gate?.kind).toBe("mediumTriage");
    expect(() => mockFlowAction(7, { type: "finishTriage" })).toThrow();
    const medium = triage.findings.find((f) => f.severity === "medium")!;
    expect(() => mockDecideFinding({ wpId: 7, fingerprint: medium.fingerprint, decision: "accept" })).toThrow();
    mockDecideFinding({ wpId: 7, fingerprint: medium.fingerprint, decision: "accept", reason: "ok" });
    mockFlowAction(7, { type: "finishTriage" });
    const push = step(7);
    expect(push.status).toBe("awaitingPush");
    expect(push.push?.blocks.map((b) => b.label)).toEqual(["Commit message", "PR description"]);
    expect(mockFlowCard(7)?.column).toBe("awaitingPush");
    expect(mockFlowAction(7, { type: "finish" }).status).toBe("done");
  });

  it("stops a running step and lets it run again", () => {
    mockFlowAction(8, { type: "run" });
    const stopped = mockFlowAction(8, { type: "stop" });
    expect(stopped.error?.code).toBe("step.stopped");
    expect(mockFlowCard(8)?.step?.state).toBe("failed");
    expect(mockFlowAction(8, { type: "retry" }).status).toBe("running");
  });
});
