import { describe, expect, it } from "vitest";
import {
  FUEL_R,
  VIEW,
  asteroidLayout,
  asteroidShape,
  atmosphereRadius,
  beltRadius,
  clampZoom,
  fuelArcs,
  handoffRingRadius,
  hashString,
  orbitRadius,
  planetPosition,
  polar,
  spiralPath,
  stepIndex,
} from "./geometry";
import { remainingPct } from "../quota/quota";
import { STEP_COUNT } from "../../shared/steps";

const NOW = Date.parse("2026-11-13T08:00:00Z");

describe("orbit geometry", () => {
  it("maps steps to growing orbits", () => {
    expect(stepIndex("intake")).toBe(1);
    expect(stepIndex("pushSummary")).toBe(STEP_COUNT);
    for (let i = 1; i < STEP_COUNT; i++) expect(orbitRadius(i + 1)).toBeGreaterThan(orbitRadius(i));
  });

  it("keeps planets and fuel ring inside the view", () => {
    for (let i = 1; i <= STEP_COUNT; i++) {
      const p = planetPosition(i);
      expect(Math.hypot(p.x, p.y)).toBeCloseTo(orbitRadius(i), 0);
      expect(Math.abs(p.x)).toBeLessThan(VIEW.w / 2);
      expect(Math.abs(p.y)).toBeLessThan(VIEW.h / 2);
    }
    expect(FUEL_R + 30).toBeLessThan(VIEW.h / 2);
  });

  it("polar uses clockwise SVG angles", () => {
    expect(polar(10, 0)).toEqual({ x: 10, y: 0 });
    expect(polar(10, 90)).toEqual({ x: 0, y: 10 });
  });

  it("draws spiral paths only forward", () => {
    expect(spiralPath(3, 3)).toBe("");
    expect(spiralPath(1, 3).startsWith("M ")).toBe(true);
  });

  it("places asteroids deterministically on the belt", () => {
    const items = [
      { fingerprint: "a", severity: "high" as const },
      { fingerprint: "b", severity: "low" as const },
    ];
    const one = asteroidLayout(items);
    const two = asteroidLayout(items);
    expect(one.get("a")).toEqual(two.get("a"));
    const a = one.get("a");
    expect(a).toBeDefined();
    if (a) expect(Math.abs(Math.hypot(a.x, a.y) - beltRadius())).toBeLessThan(10);
    expect(asteroidShape("a", 8)).toBe(asteroidShape("a", 8));
    expect(hashString("a")).not.toBe(hashString("b"));
  });

  it("grows the atmosphere with context up to the handoff ring", () => {
    expect(atmosphereRadius(0, 500_000)).toBeLessThan(atmosphereRadius(250_000, 500_000));
    expect(atmosphereRadius(500_000, 500_000)).toBe(handoffRingRadius());
    expect(atmosphereRadius(10_000_000, 500_000)).toBeLessThan(handoffRingRadius() * 1.3);
  });

  it("computes remaining quota and fuel arcs", () => {
    expect(remainingPct({ kind: "fiveHour", usedPct: 17 }, NOW)).toBe(83);
    const passed = new Date(NOW - 1000).toISOString();
    expect(remainingPct({ kind: "fiveHour", usedPct: 90, resetsAt: passed }, NOW)).toBe(100);
    const arcs = fuelArcs(
      [{ provider: "claude", windows: [{ kind: "fiveHour", usedPct: 100 }], source: "x", observedAt: "" }],
      NOW,
    );
    expect(arcs).toHaveLength(4);
    expect(arcs[0]?.fill).toBeNull();
    expect(arcs[0]?.remainingPct).toBe(0);
  });

  it("clamps zoom", () => {
    expect(clampZoom(10)).toBe(2.6);
    expect(clampZoom(0.1)).toBe(0.5);
  });
});
