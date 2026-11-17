import { afterEach, describe, expect, it } from "vitest";
import {
  mockDoneCards,
  mockFlowAction,
  mockPauseTask,
  mockPlaceCard,
  mockPushCheck,
  mockStarted,
  mockTaskDetail,
  resetMockFlows,
} from "./mockFlow";
import type { TaskCard } from "./types";

const inbox: TaskCard = { id: "WP-9", wpId: 9, title: "t", project: "p", nxStatus: "Open", column: "inbox", newComments: 0 };

afterEach(resetMockFlows);

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
