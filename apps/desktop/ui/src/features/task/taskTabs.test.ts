import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MOCK_STEP_MS,
  mockAllowBash,
  mockDecideFinding,
  mockFlowAction,
  mockFlowCard,
  mockStepChat,
  mockTaskDetail,
  resetMockFlows,
} from "../../ipc/mockFlow";
import type { FindingView, TaskDetail } from "../../ipc/types";
import { parseBlocks } from "../../shared/Markdown";
import { asTab, defaultTab, findingCounts, gateTab, sortFindings, unpicked, unpickedSevere } from "./taskTabs";

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
    expect(at({ status: "gate", gate: { kind: "findings", since: "", message: "" } })).toBe("findings");
    expect(at({ status: "gate", gate: { kind: "afterFix", since: "", message: "" } })).toBe("progress");
    expect(at({ status: "gate", gate: { kind: "error", since: "", message: "" } })).toBe("progress");
    expect(at({ status: "running" })).toBe("progress");
    expect(at({ status: "awaitingPush" })).toBe("push");
    // after a step, the timeline (not the plan) unless a gate needs another tab
    const ran = mockTaskDetail(186196).runs;
    expect(at({ status: "ready", current: "reviewLocal", plan: "## x", runs: ran })).toBe("progress");
    expect(gateTab("findings")).toBe("findings");
    expect(gateTab("afterFix")).toBe("progress");
    expect(gateTab("error")).toBe("progress");
    expect(asTab("log")).toBe("progress");
    expect(asTab("chat")).toBe("chat");
    expect(asTab("nope")).toBeUndefined();
  });

  it("sorts open findings first, most severe first, and counts only what the user picked", () => {
    const list = [
      finding({ fingerprint: "a", severity: "low" }),
      finding({ fingerprint: "b", severity: "high", state: "fixed" }),
      finding({ fingerprint: "c", severity: "medium" }),
      finding({ fingerprint: "d", severity: "critical" }),
      finding({ fingerprint: "e", severity: "low", chosen: true }),
    ];
    expect(sortFindings(list).map((f) => f.fingerprint)).toEqual(["d", "c", "a", "e", "b"]);
    // nothing is fixed by severity alone: only the low the user picked
    expect(findingCounts(list)).toEqual({ open: 4, fix: 1 });
    expect(unpickedSevere(list).map((f) => f.fingerprint)).toEqual(["d"]);
    expect(unpicked(list)).toBe(3);
    // fixed and waiting for a review: neither open nor taken again
    expect(findingCounts([finding({ state: "fixing", chosen: true })])).toEqual({ open: 0, fix: 0 });
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
    // Tự review reports a high and a low: the user picks what the AI fixes
    mockFlowAction(7, { type: "run" });
    const first = step(7);
    expect(first.gate).toMatchObject({ kind: "findings", review: "reviewLocal", next: "crossReview" });
    expect(first.findings.map((f) => f.source)).toEqual(["review-local", "review-local"]);
    expect(() => mockFlowAction(7, { type: "pickFindings", fix: true })).toThrow(/Chưa chọn/);
    const high = first.findings.find((f) => f.severity === "high")!;
    mockDecideFinding({ wpId: 7, fingerprint: high.fingerprint, decision: "fix" });
    expect(mockFlowAction(7, { type: "pickFindings", fix: true })).toMatchObject({ status: "running", current: "fix" });
    expect(mockTaskDetail(7).findings.find((f) => f.severity === "high")?.state).toBe("fixing");
    // the fix asks: Review chéo?
    expect(step(7)).toMatchObject({ status: "gate", current: "fix", gate: { kind: "afterFix", next: "finalReview" } });
    expect(mockFlowAction(7, { type: "afterFix", choice: "crossReview" })).toMatchObject({ status: "running", round: 1 });
    const cross = step(7);
    expect(cross.current).toBe("finalReview");
    expect(cross.findings.find((f) => f.severity === "high")?.state).toBe("fixed");
    // Review chốt reports a medium: its own gate, leading to the push summary
    mockFlowAction(7, { type: "run" });
    const last = step(7);
    expect(last.gate).toMatchObject({ kind: "findings", review: "finalReview", next: "pushSummary" });
    expect(last.gate?.message).toContain("Đã review chéo 1 vòng.");
    const medium = last.findings.find((f) => f.severity === "medium")!;
    expect(() => mockDecideFinding({ wpId: 7, fingerprint: medium.fingerprint, decision: "accept" })).toThrow();
    mockDecideFinding({ wpId: 7, fingerprint: medium.fingerprint, decision: "accept", reason: "ok" });
    mockFlowAction(7, { type: "pickFindings", fix: false });
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
    const runs = mockTaskDetail(8).runs;
    expect(runs.map((r) => r.state)).toEqual(["failed", "running"]);
    expect(runs[0]?.note).toContain("dừng");
  });

  it("keeps each run with its result, refused commands and transcript", () => {
    mockFlowAction(9, { type: "run" });
    expect(mockStepChat(9).run.state).toBe("running");
    step(9);
    mockFlowAction(9, { type: "approvePlan" });
    const d = step(9);
    const [analyze, implement] = d.runs;
    expect(analyze).toMatchObject({ step: "analyze", state: "done", note: "plan: 2 file sẽ sửa, 1 câu hỏi" });
    expect(analyze?.resume).toMatch(/^claude --resume /);
    expect(implement?.note).toBe("2 file thay đổi (+38 −6), 1 lệnh bị chặn");
    expect(implement?.denials[0]?.rules).toEqual([
      { rule: "npx tsc *", allowed: false },
      { rule: "npx eslint *", allowed: false },
    ]);
    const chat = mockStepChat(9);
    expect(chat.run.id).toBe(implement?.id);
    expect(chat.items[0]?.kind).toBe("prompt");
    expect(chat.items.some((i) => i.denied)).toBe(true);
    expect(chat.items.at(-1)?.kind).toBe("end");
    expect(mockStepChat(9, analyze?.id).run.step).toBe("analyze");

    expect(() => mockAllowBash(9, "git push *")).toThrow();
    expect(() => mockAllowBash(9, "npx tsc && rm -rf /")).toThrow();
    mockAllowBash(9, "Bash(npx tsc *)");
    const allowed = mockAllowBash(9, "npx eslint *");
    expect(allowed.runs[1]?.denials[0]?.rules.every((r) => r.allowed)).toBe(true);
  });
});
