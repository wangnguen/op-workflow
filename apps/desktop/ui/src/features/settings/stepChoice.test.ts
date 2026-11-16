import { describe, expect, it } from "vitest";
import { visibleAlerts } from "../../app/alerts";
import { mockSetStepChoice, mockSettings } from "../../ipc/mock";
import type { AlertView, StepSetting } from "../../ipc/types";
import { modelName, modelText } from "../../shared/models";
import { CUSTOM, effortOptions, modelOptions } from "./stepChoice";

const row = (over: Partial<StepSetting>): StepSetting => ({
  step: "fix",
  agent: "claude",
  agentChoice: false,
  runs: 0,
  ...over,
});

describe("model and effort options", () => {
  it("starts with the machine's default and ends with a custom name", () => {
    const opts = modelOptions(row({ lastModel: "claude-opus-5-5" }), "claude", []);
    expect(opts[0]).toEqual({ value: "", label: "Theo máy (lần trước: Opus 5.5)" });
    expect(opts.map((o) => o.value)).toContain("claude-sonnet-5-5");
    expect(opts.find((o) => o.value === "claude-haiku-5-5")?.label).toBe("Haiku 5.5 · $0.10 / $0.50");
    expect(opts[opts.length - 1]?.value).toBe(CUSTOM);
  });

  it("keeps a typed model and lists Codex models from its config", () => {
    const codex = modelOptions(row({ step: "crossReview", agent: "codex", model: "gpt-x" }), "codex", ["gpt-6.1-sol"]);
    expect(codex.map((o) => o.value)).toEqual(["", "gpt-6.1-sol", "gpt-x", CUSTOM]);
    expect(codex[1]?.label).toContain("mặc định của Codex");
  });

  it("effort lists follow the agent; push summary names the app's default", () => {
    expect(effortOptions(row({}), "claude").map((o) => o.value)).toEqual(["", "low", "medium", "high", "xhigh", "max"]);
    expect(effortOptions(row({ agent: "codex" }), "codex").map((o) => o.value)).not.toContain("max");
    const push = effortOptions(row({ step: "pushSummary", effortDefault: "low" }), "claude");
    expect(push[0]?.label).toBe("Mặc định của app (low)");
  });

  it("names models the short way", () => {
    expect(modelName("claude-sonnet-5-5")).toBe("Sonnet 5.5");
    expect(modelName("claude-opus-5-5-20261101")).toBe("Opus 5.5");
    expect(modelName("gpt-6.1-sol")).toBe("gpt-6.1-sol");
    expect(modelText("claude-fable-5-2", "max")).toBe("Fable 5.2 · max");
    expect(modelText(undefined, undefined)).toBe("");
  });

  it("the preview remembers picks and refuses odd names", () => {
    const fix = () => mockSettings().steps?.find((s) => s.step === "fix");
    expect(fix()?.model).toBeUndefined();
    mockSetStepChoice({ step: "fix", model: "claude-sonnet-5-5", effort: "high" });
    expect(fix()).toMatchObject({ model: "claude-sonnet-5-5", effort: "high" });
    expect(() => mockSetStepChoice({ step: "fix", model: "--yolo now" })).toThrow();
    mockSetStepChoice({ step: "fix" });
    expect(fix()?.model).toBeUndefined();
  });
});

describe("bell filter", () => {
  const alert = (id: number, read: boolean): AlertView => ({ id, at: "2026-11-14T10:00:00Z", kind: "gate", title: "t", body: "", read });
  it("shows unread only, or everything", () => {
    const items = [alert(1, true), alert(2, false)];
    expect(visibleAlerts(items, "unread").map((a) => a.id)).toEqual([2]);
    expect(visibleAlerts(items, "all").map((a) => a.id)).toEqual([1, 2]);
  });
});
