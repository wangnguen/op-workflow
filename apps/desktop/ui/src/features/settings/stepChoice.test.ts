import { describe, expect, it } from "vitest";
import { visibleAlerts } from "../../app/alerts";
import { mockSetStepChoice, mockSettings, mockStepModels } from "../../ipc/mock";
import type { AlertView, StepSetting } from "../../ipc/types";
import { modelName, modelText } from "../../shared/models";
import { effortForModel, effortOptions, modelOptions } from "./stepChoice";

const row = (over: Partial<StepSetting>): StepSetting => ({
  step: "fix",
  agent: "claude",
  agentChoice: false,
  runs: 0,
  ...over,
});
const { claude, codex } = mockStepModels();
const values = (opts: { value: string }[]) => opts.map((o) => o.value);

describe("model and effort options", () => {
  it("lists only what the CLI offers, after the machine's default", () => {
    const opts = modelOptions(row({ lastModel: "claude-opus-5-5" }), claude);
    expect(opts[0]).toEqual({ value: "", label: "Theo máy (Opus 5.5 · 1M)" });
    expect(values(opts)).toEqual(["", ...claude.models.map((m) => m.id)]);
    expect(opts.find((o) => o.value === "claude-haiku-5-5")?.label).toBe("Haiku 5.5");
    expect(values(modelOptions(row({ agent: "codex" }), codex))).toEqual(["", "gpt-6.1-sol", "gpt-6-luna"]);
  });

  it("while the list loads, shows only what is picked now", () => {
    expect(modelOptions(row({ lastModel: "claude-opus-5-5" }), undefined)).toEqual([
      { value: "", label: "Theo máy (lần trước: Opus 5.5)" },
    ]);
    expect(values(modelOptions(row({ model: "claude-sonnet-5-5" }), undefined))).toEqual(["", "claude-sonnet-5-5"]);
    expect(effortOptions(row({ effort: "high" }), undefined)).toEqual([
      { value: "", label: "Theo máy" },
      { value: "high", label: "high" },
    ]);
  });

  it("shows a saved model the CLI no longer lists", () => {
    const opts = modelOptions(row({ step: "crossReview", agent: "codex", model: "gpt-x" }), codex);
    expect(values(opts)).toEqual(["", "gpt-6.1-sol", "gpt-6-luna", "gpt-x"]);
    expect(opts[0]?.label).toBe("Theo máy (GPT-6.1-Sol)");
    expect(opts[3]?.label).toBe("gpt-x (máy không còn model này)");
  });

  it("effort levels follow the model", () => {
    const all = effortOptions(row({}), claude);
    expect(values(all)).toEqual(["", "low", "medium", "high", "xhigh", "max"]);
    expect(all[0]?.label).toBe("Theo máy (high)");
    expect(effortOptions(row({ model: "claude-haiku-4-5-20251001" }), claude)).toEqual([
      { value: "", label: "Model này không nhận effort" },
    ]);
    expect(values(effortOptions(row({ model: "claude-opus-4-6" }), claude))).not.toContain("xhigh");
    const odd = effortOptions(row({ model: "claude-opus-4-6", effort: "xhigh" }), claude);
    expect(odd[odd.length - 1]).toEqual({ value: "xhigh", label: "xhigh (model này không có)" });
    expect(values(effortOptions(row({ agent: "codex" }), codex))).toContain("ultra");
    // a saved name the list no longer has: every level some model takes
    expect(values(effortOptions(row({ agent: "codex", model: "gpt-x" }), codex))).toContain("ultra");
    const push = effortOptions(row({ step: "pushSummary", effortDefault: "low" }), claude);
    expect(push[0]?.label).toBe("Mặc định của app (low)");
  });

  it("a new model keeps the effort only if it takes it", () => {
    expect(effortForModel(claude, "high", "claude-opus-4-6")).toBe("high");
    expect(effortForModel(claude, "xhigh", "claude-opus-4-6")).toBeUndefined();
    expect(effortForModel(claude, "low", "claude-haiku-4-5-20251001")).toBeUndefined();
    expect(effortForModel(claude, "xhigh", undefined)).toBe("xhigh");
    expect(effortForModel(undefined, "xhigh", "claude-opus-4-6")).toBe("xhigh");
  });

  it("names models the short way", () => {
    expect(modelName("claude-sonnet-5-5")).toBe("Sonnet 5.5");
    expect(modelName("claude-opus-5-5-20261101")).toBe("Opus 5.5");
    expect(modelName("gpt-6.1-sol")).toBe("gpt-6.1-sol");
    expect(modelName("claude-haiku-4-5-20251001")).toBe("Haiku 4.5");
    expect(modelName("claude-sonnet-5")).toBe("Sonnet 5");
    expect(modelName("claude-sonnet-5-20261101")).toBe("Sonnet 5");
    expect(modelName("claude-opus-5-5[1m]")).toBe("Opus 5.5");
    expect(modelText("claude-fable-5-2", "max")).toBe("Fable 5.2 · max");
    expect(modelText(undefined, undefined)).toBe("");
  });

  it("the preview remembers picks and refuses names the CLI does not list", () => {
    const fix = () => mockSettings().steps?.find((s) => s.step === "fix");
    expect(fix()?.model).toBeUndefined();
    mockSetStepChoice({ step: "fix", model: "claude-sonnet-5-5", effort: "high" });
    expect(fix()).toMatchObject({ model: "claude-sonnet-5-5", effort: "high" });
    expect(() => mockSetStepChoice({ step: "fix", model: "--yolo now" })).toThrow();
    expect(() => mockSetStepChoice({ step: "fix", model: "claude-opus-9" })).toThrow();
    expect(() => mockSetStepChoice({ step: "crossReview", model: "claude-sonnet-5-5" })).toThrow();
    expect(() => mockSetStepChoice({ step: "fix", model: "claude-haiku-4-5-20251001", effort: "low" })).toThrow();
    expect(() => mockSetStepChoice({ step: "fix", model: "claude-opus-4-6", effort: "xhigh" })).toThrow();
    mockSetStepChoice({ step: "fix", model: "claude-opus-4-6", effort: "max" });
    expect(() => mockSetStepChoice({ step: "crossReview", effort: "ultra" })).not.toThrow();
    mockSetStepChoice({ step: "crossReview" });
    mockSetStepChoice({ step: "fix" });
    expect(fix()?.model).toBeUndefined();
  });
});

describe("bell filter", () => {
  const alert = (id: number, read: boolean): AlertView => ({ id, at: "2026-11-14T10:00:00Z", kind: "gate", title: "t", body: "", read });
  it("shows unread only, or everything, newest first", () => {
    const items = [alert(1, true), alert(2, false)];
    expect(visibleAlerts(items, "unread").map((a) => a.id)).toEqual([2]);
    expect(visibleAlerts(items, "all").map((a) => a.id)).toEqual([2, 1]);
  });
});
