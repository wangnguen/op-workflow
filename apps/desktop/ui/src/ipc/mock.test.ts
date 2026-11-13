import { describe, expect, it } from "vitest";
import { STEP_COUNT } from "../shared/steps";
import { DEMO_FRAMES, boardForFrame, orbitForFrame } from "./mock";

const NOW = Date.parse("2026-11-13T08:00:00Z");

describe("demo timeline", () => {
  it("every frame yields a consistent orbit and board", () => {
    DEMO_FRAMES.forEach((_, i) => {
      const orbit = orbitForFrame(i, NOW);
      const board = boardForFrame(i, NOW);
      const activeCards = board.tasks.filter((t) => t.column === "active");
      expect(activeCards.length).toBeLessThanOrEqual(1);
      if (orbit.active) {
        expect(board.activeTaskId).toBe(orbit.active.taskId);
        expect(orbit.active.steps).toHaveLength(STEP_COUNT);
        const live = orbit.active.steps.filter((s) => s.state === "running" || s.state === "waitingUser");
        expect(live).toHaveLength(1);
        orbit.active.steps.forEach((s, idx) => expect(s.index).toBe(idx + 1));
      } else {
        expect(board.activeTaskId).toBeUndefined();
        expect(orbit.inbox.length).toBeGreaterThan(0);
      }
      for (const q of orbit.quota) {
        for (const w of q.windows) {
          expect(w.usedPct).toBeGreaterThanOrEqual(0);
          expect(w.usedPct).toBeLessThanOrEqual(100);
        }
      }
    });
  });

  it("covers the story: gate, compaction, codex child, review round 2", () => {
    const frames = DEMO_FRAMES.map((_, i) => orbitForFrame(i, NOW).active);
    expect(frames.some((a) => a?.gate?.kind === "planReview")).toBe(true);
    expect(frames.some((a) => a?.gate?.kind === "mediumTriage")).toBe(true);
    expect(frames.some((a) => (a?.agents[0]?.compactions ?? 0) > 0)).toBe(true);
    expect(frames.some((a) => a?.agents[0]?.children.some((c) => c.kind === "codex"))).toBe(true);
    expect(frames.some((a) => a?.steps.find((s) => s.kind === "crossReview")?.round === 2)).toBe(true);
    expect(frames.some((a) => a?.findings.some((f) => f.state === "fixed"))).toBe(true);
    expect(frames.some((a) => a === undefined)).toBe(true);
  });
});

describe("settings mock and errors", () => {
  it("rejects an empty token and reports the message", async () => {
    const { mockSaveToken, mockClearToken, mockSettings } = await import("./mock");
    const { errorMessage } = await import("./client");
    expect(mockSettings().nxBaseUrl).toBe("");
    expect(() => mockSaveToken("op.example.com", "abc")).toThrow();
    try {
      mockSaveToken("https://op.example.com", "  ");
    } catch (e) {
      expect(errorMessage(e)).toBe("Token trống");
    }
    expect(mockSaveToken("https://op.example.com/", "abc").nxTokenSet).toBe(true);
    expect(mockSettings().nxBaseUrl).toBe("https://op.example.com");
    expect(mockClearToken().nxTokenSet).toBe(false);
  });
});
