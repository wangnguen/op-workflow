import { describe, expect, it } from "vitest";
import {
  FUEL_R,
  VIEW,
  asteroidLayout,
  asteroidShape,
  atmosphereRadius,
  beltRadius,
  clampZoom,
  distantPositions,
  fuelArcs,
  handoffRingRadius,
  hashString,
  labelPosition,
  DRIFT_DEG,
  labelWidth,
  handoffRingRadius as ringR,
  COMMENT_BADGE_BOX,
  STAR_LABEL_BOX,
  boxesHit,
  gateLabelBox,
  gateLabelOffset,
  gatePosition,
  shipLabelBox,
  shipLabelOffset,
  shipPosition,
  nebulaLayout,
  planetDrift,
  orbitRadius,
  planetPosition,
  polar,
  spiralPath,
  stepIndex,
} from "./geometry";
import { remainingPct } from "../quota/quota";
import { stepLabel } from "../../i18n/vi";
import { mockQuota } from "../../ipc/mock";
import { STEP_ORDER } from "../../shared/steps";
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

  it("spreads any number of inbox nebulae without piling them up", () => {
    for (const n of [1, 3, 6, 7, 12, 25, 40]) {
      const spots = nebulaLayout(n);
      expect(spots).toHaveLength(n);
      const scale = spots[0]?.scale ?? 1;
      expect(scale).toBeGreaterThanOrEqual(0.55);
      if (n <= 6) expect(scale).toBe(1);
      // the empty centre (protostar + hint) stays clear
      for (const p of spots) expect(Math.hypot(p.x, p.y)).toBeGreaterThanOrEqual(200);
      for (let i = 0; i < n; i++) {
        for (let j = i + 1; j < n; j++) {
          const a = spots[i];
          const b = spots[j];
          if (!a || !b) continue;
          expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeGreaterThan(110 * scale);
        }
      }
    }
  });

  it("shows every outside session, away from the fuel gauge", () => {
    expect(distantPositions(4)).toHaveLength(4);
    for (const n of [7, 14, 30]) {
      const spots = distantPositions(n);
      expect(spots).toHaveLength(n);
      for (const p of spots) {
        // the gauge and its labels fill the top half
        if (p.y < -130) expect(Math.hypot(p.x, p.y)).toBeGreaterThan(FUEL_R + 100);
        // outside the outermost orbit, inside the map
        expect(Math.hypot(p.x, p.y)).toBeGreaterThan(orbitRadius(STEP_COUNT) + 15);
        expect(Math.abs(p.x)).toBeLessThanOrEqual(VIEW.w / 2);
        expect(Math.abs(p.y)).toBeLessThanOrEqual(VIEW.h / 2);
      }
      const keys = new Set(spots.map((p) => `${p.x},${p.y}`));
      expect(keys.size).toBe(n);
    }
  });

  it("clamps zoom", () => {
    expect(clampZoom(10)).toBe(2.6);
    expect(clampZoom(0.1)).toBe(0.5);
  });

  it("turns the whole system together: a full turn lands where it started", () => {
    for (let i = 1; i <= STEP_COUNT; i++) {
      const a = planetPosition(i, 0);
      const b = planetPosition(i, 360);
      expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeLessThan(0.05);
      expect(Math.hypot(...Object.values(planetPosition(i, 90)))).toBeCloseTo(orbitRadius(i), 1);
    }
    expect(spiralPath(1, 3, 14, 30)).not.toBe(spiralPath(1, 3, 14, 0));
  });

  it("moves planet labels smoothly while the system turns and keeps them off the planet", () => {
    for (let i = 1; i <= STEP_COUNT; i++) {
      let prev = labelPosition(i, 0, 90);
      for (let rot = 1; rot <= 360; rot++) {
        const l = labelPosition(i, rot, 90);
        // one degree never makes a label jump to the other side
        expect(Math.hypot(l.x - prev.x, l.y - prev.y)).toBeLessThan(12);
        const p = planetPosition(i, rot);
        expect(Math.hypot(l.x - p.x, l.y - p.y)).toBeGreaterThan(15);
        prev = l;
      }
    }
  });

  it("drifts planets without labels ever covering each other or the fuel gauge labels", () => {
    type Box = { l: number; r: number; t: number; b: number; name: string };
    const hit = (a: Box, b: Box) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b;
    const fuel: Box[] = fuelArcs(mockQuota(0, NOW), NOW).map((a) => {
      const w = 118;
      const l = a.labelAnchor === "start" ? a.label.x : a.label.x - w;
      return { l, r: l + w, t: a.label.y - 13, b: a.label.y + 5, name: a.key };
    });
    // the longest second line a planet can show
    const sub = "đang chạy · vòng 2";
    for (let t = 0; t <= 1200; t += 1.5) {
      const drift = planetDrift(t);
      const boxes: Box[] = STEP_ORDER.map((kind, i) => {
        const index = i + 1;
        const main = `${index}. ${stepLabel[kind]}`;
        const w = labelWidth(main, sub);
        const c = labelPosition(index, drift, w);
        return { l: c.x - w / 2, r: c.x + w / 2, t: c.y - 13, b: c.y + 15, name: main };
      });
      for (const [i, a] of boxes.entries()) {
        for (const b of boxes.slice(i + 1)) {
          expect(hit(a, b), `${a.name} / ${b.name} at t=${t}`).toBe(false);
        }
        for (const f of fuel) expect(hit(a, f), `${a.name} / ${f.name} at t=${t}`).toBe(false);
      }
      expect(Math.abs(drift(1))).toBeLessThanOrEqual(DRIFT_DEG);
      // the ship (on any step) keeps its label clear of every planet label and the star's name
      for (let orbit = 1; orbit <= STEP_ORDER.length; orbit++) {
        const ship = shipLabelBox(shipPosition(orbit, drift), shipLabelOffset(orbit, ringR(), drift));
        for (const [i, b] of boxes.entries()) {
          if (i + 1 === orbit) continue;
          expect(boxesHit(ship, b), `ship@${orbit} / ${b.name} at t=${t}`).toBe(false);
        }
        expect(boxesHit(ship, STAR_LABEL_BOX), `ship@${orbit} / star at t=${t}`).toBe(false);
        expect(boxesHit(ship, COMMENT_BADGE_BOX), `ship@${orbit} / comment badge at t=${t}`).toBe(false);
      }
      // the gate station (on any step) keeps its label clear too
      for (let step = 1; step <= STEP_ORDER.length; step++) {
        const gate = gateLabelBox(gatePosition(step, drift), gateLabelOffset(step, drift));
        for (const b of boxes) expect(boxesHit(gate, b), `gate@${step} / ${b.name} at t=${t}`).toBe(false);
        expect(boxesHit(gate, STAR_LABEL_BOX), `gate@${step} / star at t=${t}`).toBe(false);
      }
    }
  });
});
