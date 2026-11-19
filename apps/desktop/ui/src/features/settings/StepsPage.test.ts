import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { mockSetStepChoice, mockSetStepLimits, mockSetVerifyMcp, mockSettings, mockStepModels } from "../../ipc/mock";
import { vi as text } from "../../i18n/vi";
import { StepsPage } from "./StepsPage";

afterEach(() => {
  mockSetVerifyMcp(true);
  mockSetStepLimits({ step: "analyze" });
  mockSetStepChoice({ step: "analyze" });
});

describe("AI step verification and limits", () => {
  it("saves the switch and limits independently of model and permission picks", () => {
    mockSetStepChoice({ step: "analyze", model: "claude-sonnet-5-5", effort: "high", permissionMode: "plan" });
    mockSetVerifyMcp(false);
    const view = mockSetStepLimits({ step: "analyze", maxBudgetUsd: 8, timeoutMin: 42 });
    expect(view.verifyMcp).toBe(false);
    expect(view.steps?.find((s) => s.step === "analyze")).toMatchObject({ maxBudgetUsd: 8, timeoutMin: 42, model: "claude-sonnet-5-5", effort: "high", permissionMode: "plan" });
    mockSetStepChoice({ step: "analyze", agent: "codex" });
    expect(mockSettings().steps?.find((s) => s.step === "analyze")).toMatchObject({ maxBudgetUsd: 8, timeoutMin: 42 });
    const cleared = mockSetStepLimits({ step: "analyze" }).steps?.find((s) => s.step === "analyze");
    expect(cleared?.maxBudgetUsd).toBeUndefined();
    expect(cleared?.timeoutMin).toBeUndefined();
    expect(cleared).toMatchObject({ defaultBudgetUsd: 5, defaultTimeoutMin: 20 });
  });

  it("rejects invalid amounts and durations", () => {
    for (const maxBudgetUsd of [-1, 0, Number.NaN, Infinity]) expect(() => mockSetStepLimits({ step: "analyze", maxBudgetUsd })).toThrow();
    for (const timeoutMin of [-1, 0, 1.5, 4294967296]) expect(() => mockSetStepLimits({ step: "analyze", timeoutMin })).toThrow();
  });

  it("renders explanations below choices and default limit placeholders in both switch states", () => {
    for (const enabled of [true, false]) {
      const html = renderToStaticMarkup(createElement(StepsPage, { view: mockSetVerifyMcp(enabled), onSaved: () => {}, models: mockStepModels(), modelsBusy: false, onReadModels: () => {} }));
      expect(html).toContain('role="switch"');
      expect(html).toContain(text.settings.steps.verifyMcp);
      expect(html).toContain(enabled ? text.settings.steps.mcpOn : text.settings.steps.mcpOff);
      expect(html).toContain('class="set-mcp-hints"');
      expect(html).toContain('placeholder="5"');
      expect(html).toContain('placeholder="20"');
      expect(html).toContain(text.settings.steps.budgetHint);
      expect(html).toContain(text.settings.steps.timeoutHint);
      expect(html).toContain(text.settings.steps.modelHint);
    }
  });
});
