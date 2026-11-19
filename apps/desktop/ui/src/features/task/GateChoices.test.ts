import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MOCK_STEP_MS, mockFlowAction, mockSaveBrief, mockTaskDetail, resetMockFlows } from "../../ipc/mockFlow";
import { BudgetChoices, PlanChangedChoices } from "./GateChoices";

afterEach(() => { vi.useRealTimers(); resetMockFlows(); });

describe("workflow follow-up choices", () => {
  it("renders both brief decisions with visible explanations", () => {
    const html = renderToStaticMarkup(createElement(PlanChangedChoices, { disabled: false, onAction: () => {} }));
    expect(html).toContain("Phân tích lại");
    expect(html).toContain("Giữ plan, chạy tiếp");
    expect(html.match(/<small/g)).toHaveLength(2);
    expect(html).toContain("AI nhận plan cũ");
  });
  it("shows the current extra budget and explains why continuation may be disabled", () => {
    for (const canGoOn of [false, true]) {
      const html = renderToStaticMarkup(createElement(BudgetChoices, { disabled: false, canGoOn, budget: 7, onAction: () => {} }));
      expect(html).toContain("Chạy tiếp (thêm $7)");
      expect(html).toContain("Chạy lại từ đầu");
      expect(html.match(/<small/g)).toHaveLength(2);
      expect(html).toContain(canGoOn ? "Cài đặt → Các bước AI" : "Phiên vừa dừng không còn");
    }
  });
  it("keeps the pending workflow decision or returns to analysis after the brief changes", () => {
    vi.useFakeTimers();
    mockFlowAction(9, { type: "run" });
    vi.advanceTimersByTime(MOCK_STEP_MS + 10);
    for (const q of mockTaskDetail(9).questions) mockFlowAction(9, { type: "answerQuestion", id: q.id, aiDecides: true });
    mockFlowAction(9, { type: "approvePlan" });
    vi.advanceTimersByTime(MOCK_STEP_MS * 4 + 10);
    mockFlowAction(9, { type: "setAuto", on: false });
    const before = mockTaskDetail(9);
    const changed = mockSaveBrief(9, `${before.brief}\nNew requirement`);
    expect(changed.error?.code).toBe("plan.briefChanged");
    expect(() => mockFlowAction(9, { type: "run" })).toThrow();
    const kept = mockFlowAction(9, { type: "keepPlan" });
    expect(kept.error).toBeUndefined();
    expect(kept.log.some((l) => l.text === "Bạn giữ plan sau khi brief đổi")).toBe(true);
    if (kept.status === "running") { vi.advanceTimersByTime(MOCK_STEP_MS * 4 + 10); mockTaskDetail(9); }
    mockSaveBrief(9, `${changed.brief}\nAnother requirement`);
    const again = mockFlowAction(9, { type: "reopen", step: "analyze" });
    expect(again.current).toBe("analyze");
    expect(again.error).toBeUndefined();
    expect(again.plan).toBeTruthy();
  });
});
