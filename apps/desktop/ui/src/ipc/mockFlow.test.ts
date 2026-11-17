import { afterEach, describe, expect, it, vi } from "vitest";
import {
  MOCK_STEP_MS,
  mockDecideFinding,
  mockDoneCards,
  mockFlowAction,
  mockPauseTask,
  mockPlaceCard,
  mockPushCheck,
  mockStarted,
  mockTaskDetail,
  resetMockFlows,
} from "./mockFlow";
import type { StepKind, StepState, TaskCard, TaskDetail } from "./types";

const inbox: TaskCard = { id: "WP-9", wpId: 9, title: "t", project: "p", nxStatus: "Open", column: "inbox", newComments: 0 };

afterEach(() => {
  resetMockFlows();
  vi.useRealTimers();
});

/** Let the running mock step finish, then read the task. */
function finishStep(wpId: number): TaskDetail {
  vi.setSystemTime(Date.now() + MOCK_STEP_MS + 10);
  return mockTaskDetail(wpId);
}

const stateOf = (d: TaskDetail, kind: StepKind): StepState | undefined => d.steps.find((s) => s.kind === kind)?.state;

describe("12-step flow in the preview", () => {
  it("passes the steps not turned on, asks after every fix and runs Review chốt again after its fix", () => {
    vi.useFakeTimers();
    mockStarted(9, "t");
    mockFlowAction(9, { type: "run" });
    expect(finishStep(9).gate?.kind).toBe("planReview");
    mockFlowAction(9, { type: "approvePlan" });

    // Viết code → Kiểm tra and Harness are passed ("bỏ qua") → Tự review
    let d = finishStep(9);
    expect([d.status, d.current]).toEqual(["ready", "reviewLocal"]);
    expect([stateOf(d, "checks"), stateOf(d, "harness")]).toEqual(["skipped", "skipped"]);
    expect(stateOf(d, "browserTest")).toBe("pending");

    // Tự review finds a high → the user picks it → the first Sửa lỗi (7) → "Review chéo?"
    mockFlowAction(9, { type: "run" });
    expect(finishStep(9).gate?.kind).toBe("findings");
    mockDecideFinding({ wpId: 9, fingerprint: "m-high", decision: "fix" });
    mockFlowAction(9, { type: "pickFindings", fix: true });
    d = finishStep(9);
    expect([d.current, d.gate?.kind, stateOf(d, "fix")]).toEqual(["fix", "afterFix", "waitingUser"]);
    expect(d.runs.at(-1)?.note).toBe("lần sửa thứ 1: xử lý 1 lỗi; 2 file thay đổi (+41 −6)");
    d = mockFlowAction(9, { type: "afterFix", choice: "crossReview" });
    expect([d.current, d.steps.find((s) => s.kind === "crossReview")?.round]).toEqual(["crossReview", 1]);

    // Review chéo clean → Review chốt finds a medium → its gate
    expect(finishStep(9).current).toBe("finalReview");
    mockFlowAction(9, { type: "run" });
    d = finishStep(9);
    expect(d.gate).toMatchObject({ kind: "findings", review: "finalReview" });

    // fixing it runs the second Sửa lỗi (9); "No" runs Review chốt again, not Review chéo
    mockDecideFinding({ wpId: 9, fingerprint: "m-med", decision: "fix" });
    d = mockFlowAction(9, { type: "pickFindings", fix: true });
    expect([d.status, d.current]).toEqual(["running", "fixCross"]);
    d = finishStep(9);
    expect(d.gate).toMatchObject({ kind: "afterFix", review: "finalReview", next: "finalReview" });
    d = mockFlowAction(9, { type: "afterFix", choice: "next" });
    expect([d.status, d.current, d.round]).toEqual(["running", "finalReview", 1]);
    expect(stateOf(d, "fixCross")).toBe("done");

    // clean Review chốt → Test trình duyệt passed → Push summary
    d = finishStep(9);
    expect([d.status, d.current]).toEqual(["ready", "pushSummary"]);
    expect(stateOf(d, "browserTest")).toBe("skipped");
    expect(d.steps.map((s) => s.index)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
  });

  it("picks more after a fix, or skips the cross review", () => {
    vi.useFakeTimers();
    mockStarted(9, "t");
    mockFlowAction(9, { type: "run" });
    finishStep(9);
    mockFlowAction(9, { type: "approvePlan" });
    finishStep(9);
    mockFlowAction(9, { type: "run" });
    finishStep(9);
    // fix only the high; the low is still there to pick afterwards
    mockDecideFinding({ wpId: 9, fingerprint: "m-high", decision: "fix" });
    mockFlowAction(9, { type: "pickFindings", fix: true });
    finishStep(9);
    let d = mockFlowAction(9, { type: "afterFix", choice: "pickMore" });
    expect(d.gate).toMatchObject({ kind: "findings", review: "reviewLocal" });
    expect(d.findings.find((f) => f.fingerprint === "m-low")?.state).toBe("open");
    mockDecideFinding({ wpId: 9, fingerprint: "m-low", decision: "fix" });
    mockFlowAction(9, { type: "pickFindings", fix: true });
    expect(finishStep(9).runs.at(-1)?.note).toMatch(/^lần sửa thứ 2: xử lý 1 lỗi/);
    // no cross review: Review chéo and step 9 "bỏ qua", Review chốt runs
    d = mockFlowAction(9, { type: "afterFix", choice: "next" });
    expect([d.current, d.round, stateOf(d, "crossReview"), stateOf(d, "fixCross")]).toEqual([
      "finalReview",
      0,
      "skipped",
      "skipped",
    ]);
    expect(() => mockFlowAction(9, { type: "afterFix", choice: "next" })).toThrow();
  });

  it("passes a Sửa lỗi that was not needed instead of leaving it 'chưa tới'", () => {
    vi.useFakeTimers();
    const [card] = mockDoneCards();
    const d = mockTaskDetail(card!.wpId);
    expect(stateOf(d, "fix")).toBe("skipped");
    expect(stateOf(d, "fixCross")).toBe("skipped");
    expect(stateOf(d, "pushSummary")).toBe("done");
  });
});

describe("preview board moves", () => {
  it("starts, pauses, finishes early and carries on like the core", () => {
    mockStarted(9, "t");
    expect(mockPlaceCard(inbox).column).toBe("active");
    mockPauseTask(9);
    expect(mockPlaceCard(inbox).column).toBe("inbox");
    mockStarted(9, "t");

    const done = mockFlowAction(9, { type: "finish" });
    expect(done.status).toBe("done");
    expect(done.log[done.log.length - 1]?.text).toContain("bỏ qua các bước còn lại");
    expect(mockPlaceCard(inbox).column).toBe("done");
    expect(() => mockFlowAction(9, { type: "finish" })).toThrow();

    // back where it stopped: Phân tích, ready
    const back = mockFlowAction(9, { type: "reopen" });
    expect([back.status, back.current]).toEqual(["ready", "analyze"]);
    expect(mockPlaceCard(inbox).column).toBe("active");
    // Viết code was never reached
    expect(() => mockFlowAction(9, { type: "reopen", step: "implement" })).toThrow(/chưa tới bước Viết code/);
  });

  it("refuses to pause or finish while a step runs", () => {
    mockStarted(9, "t");
    mockFlowAction(9, { type: "run" });
    expect(() => mockPauseTask(9)).toThrow();
    expect(() => mockFlowAction(9, { type: "finish", forced: true })).toThrow();
  });

  it("finds the push on the second check and keeps the forced mark on a done card", () => {
    const [pushed, forced] = mockDoneCards();
    expect(pushed?.pushCheck?.pushed).toBe(true);
    expect(forced?.pushCheck?.forced).toBe(true);
    expect(forced?.nxOpen).toBe(true);
    expect(mockDoneCards(0)).toEqual([]);

    mockStarted(9, "t");
    expect(mockPushCheck(9).pushed).toBe(false);
    expect(mockPushCheck(9).pushed).toBe(true);
  });

  it("keeps a finished task's history in the Inbox and carries on from the step picked at start", () => {
    const [card] = mockDoneCards();
    mockPauseTask(card!.wpId);
    const parked = mockPlaceCard(card!);
    expect(parked.column).toBe("inbox");
    // where it stopped and its branch stay on the Inbox card
    expect(parked.step?.kind).toBe("pushSummary");
    expect(parked.finishedAt).toBeDefined();
    expect(parked.branch).toBeDefined();
    // the task window reads the history but runs nothing while parked
    expect(mockTaskDetail(card!.wpId).parked).toBe(true);
    expect(() => mockFlowAction(card!.wpId, { type: "run" })).toThrow();
    mockStarted(card!.wpId, card!.title, { step: "implement", note: "khách mở lại" });
    const again = mockPlaceCard(card!);
    expect(again.column).toBe("active");
    expect(again.step?.kind).toBe("implement");
    const d = mockTaskDetail(card!.wpId);
    expect(d.parked).toBe(false);
    expect(d.steps.find((s) => s.kind === "analyze")?.state).toBe("done");
  });

  it("starting a finished task again with no choice keeps it at the push summary", () => {
    const [, card] = mockDoneCards();
    mockPauseTask(card!.wpId);
    mockStarted(card!.wpId, card!.title);
    expect(mockPlaceCard(card!).column).toBe("awaitingPush");
  });

  it("sends a finished task back to Viết code with a note", () => {
    const [card] = mockDoneCards();
    const d = mockFlowAction(card!.wpId, { type: "reopen", step: "implement", note: "thêm test" });
    expect([d.status, d.current]).toEqual(["ready", "implement"]);
    expect(d.steps.find((s) => s.kind === "pushSummary")?.state).toBe("pending");
    expect(d.log[d.log.length - 1]?.text).toBe("Làm tiếp: chạy lại từ Viết code, kèm ghi chú: thêm test");
  });
});
