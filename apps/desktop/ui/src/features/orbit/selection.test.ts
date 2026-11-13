import { describe, expect, it } from "vitest";
import { DEMO_FRAMES, orbitForFrame } from "../../ipc/mock";
import { autoSelection, findAgent, isValidSelection, resolveSelection } from "./selection";

const NOW = Date.parse("2026-11-13T08:00:00Z");
const frame = (i: number) => orbitForFrame(i, NOW);

describe("orbit selection", () => {
  it("prefers the gate, then the agent, then the star", () => {
    expect(autoSelection(frame(2))).toEqual({ kind: "gate" });
    expect(autoSelection(frame(3))).toEqual({ kind: "agent", id: "claude-main" });
    expect(autoSelection(frame(0))).toEqual({ kind: "star" });
  });

  it("falls back to a nebula when no task is active", () => {
    const idle = frame(DEMO_FRAMES.length - 1);
    expect(idle.active).toBeUndefined();
    expect(autoSelection(idle)).toEqual({ kind: "nebula", taskId: "WP-188001" });
  });

  it("drops selections whose target disappeared", () => {
    const withFindings = frame(10);
    const fp = withFindings.active?.findings[0]?.fingerprint ?? "";
    expect(isValidSelection({ kind: "finding", fingerprint: fp }, withFindings)).toBe(true);
    expect(resolveSelection({ kind: "finding", fingerprint: fp }, frame(1))).toEqual({ kind: "agent", id: "claude-main" });
  });

  it("finds child agents", () => {
    const review = frame(10);
    expect(findAgent(review.active?.agents ?? [], "codex-review")?.kind).toBe("codex");
  });
});
