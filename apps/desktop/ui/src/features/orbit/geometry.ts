// Pure geometry for the orbit scene. Coordinates are SVG user units with the
// star at (0, 0); angles are degrees, 0° = east, growing clockwise (SVG y-down).
import type { QuotaSnapshot, QuotaWindow, Severity, StepKind } from "../../ipc/types";
import { remainingPct } from "../quota/quota";
import { STEP_ORDER } from "../../shared/steps";

export interface Point {
  x: number;
  y: number;
}

export const VIEW = { x: -480, y: -400, w: 960, h: 800 } as const;
export const STAR_R = 24;
export const ORBIT_BASE = 82;
// 8 orbits: the outermost (82 + 7 * 35 = 327) stays inside the fuel ring.
export const ORBIT_GAP = 35;
export const FUEL_R = 362;
export const PLANET_R = 8;
const JOURNEY_START_DEG = -122;
const JOURNEY_STEP_DEG = 42;
const FUEL_SPAN_DEG = 34;
export const ZOOM_MIN = 0.5;
export const ZOOM_MAX = 2.6;

export const STEPS: readonly StepKind[] = STEP_ORDER;

const round2 = (n: number) => Math.round(n * 100) / 100;

export function polar(r: number, deg: number): Point {
  const a = (deg * Math.PI) / 180;
  return { x: round2(r * Math.cos(a)), y: round2(r * Math.sin(a)) };
}

/** 1-based step index (1 = Intake … 8 = Push summary). */
export function stepIndex(kind: StepKind): number {
  return STEPS.indexOf(kind) + 1;
}

export function orbitRadius(index: number): number {
  return ORBIT_BASE + (index - 1) * ORBIT_GAP;
}

/** Planets form an outward spiral: the journey reads clockwise from upper-left. */
export function planetAngle(index: number): number {
  return JOURNEY_START_DEG + (index - 1) * JOURNEY_STEP_DEG;
}

export function planetPosition(index: number): Point {
  return polar(orbitRadius(index), planetAngle(index));
}

const SHIP_LEAD = 62;
const GATE_LAG = 50;
const arcDeg = (length: number, r: number) => (length / r) * (180 / Math.PI);

/** Ship rides its step's orbit a fixed arc length ahead of the planet (toward open space). */
export function shipAngle(index: number): number {
  return planetAngle(index) + arcDeg(SHIP_LEAD, orbitRadius(index));
}

export function shipPosition(index: number): Point {
  return polar(orbitRadius(index), shipAngle(index));
}

/** Heading of the ship (tangent of the clockwise orbit), in degrees. */
export function shipHeading(index: number): number {
  return shipAngle(index) + 90;
}

/** Ship label just outside the handoff ring, on the side facing away from the star. */
export function shipLabelOffset(index: number, ringRadius: number): Point & { anchor: "start" | "end" | "middle" } {
  const p = polar(ringRadius + 14, shipAngle(index));
  const anchor = Math.abs(p.x) < 12 ? "middle" : p.x > 0 ? "start" : "end";
  return { x: p.x, y: p.y + (p.y > 0 ? 8 : 0), anchor };
}

/** Gate station sits on the orbit just behind the planet. */
export function gatePosition(index: number): Point {
  return polar(orbitRadius(index), planetAngle(index) - arcDeg(GATE_LAG, orbitRadius(index)));
}

/** Gate label offset (relative to the gate), pointing away from its planet. */
export function gateLabelOffset(index: number): Point & { anchor: "start" | "end" } {
  const g = gatePosition(index);
  const p = planetPosition(index);
  const dx = g.x - p.x;
  const dy = g.y - p.y;
  const len = Math.hypot(dx, dy) || 1;
  return {
    x: round2((dx / len) * 24),
    y: round2((dy / len) * 24 + 4),
    anchor: dx >= 0 ? "start" : "end",
  };
}

export interface LabelLayout extends Point {
  anchor: "start" | "end" | "middle";
  /** baseline offsets (from the anchor point) of the two text lines */
  mainDy: number;
  subDy: number;
}

/**
 * Two-line planet label pushed outward from the star. Planets near the top or
 * bottom get a centred label stacked away from the planet; side planets get a
 * left/right aligned one.
 */
export function labelPosition(index: number): LabelLayout {
  const angle = planetAngle(index);
  const sin = Math.sin((angle * Math.PI) / 180);
  if (sin < -0.5) {
    const p = polar(orbitRadius(index) + 16, angle);
    return { ...p, anchor: "middle", mainDy: -14, subDy: 0 };
  }
  if (sin > 0.5) {
    const p = polar(orbitRadius(index) + 16, angle);
    return { ...p, anchor: "middle", mainDy: 12, subDy: 26 };
  }
  const p = polar(orbitRadius(index) + 18, angle);
  return { ...p, anchor: p.x > 0 ? "start" : "end", mainDy: 3, subDy: 17 };
}

export function arcPath(r: number, startDeg: number, endDeg: number): string {
  const s = polar(r, startDeg);
  const e = polar(r, endDeg);
  const large = Math.abs(endDeg - startDeg) > 180 ? 1 : 0;
  const sweep = endDeg >= startDeg ? 1 : 0;
  return `M ${s.x} ${s.y} A ${r} ${r} 0 ${large} ${sweep} ${e.x} ${e.y}`;
}

/** Spiral flight path from planet `from` to planet `to` (indices, from < to). */
export function spiralPath(from: number, to: number, samples = 14): string {
  if (to <= from) return "";
  const pts: Point[] = [];
  for (let seg = from; seg < to; seg++) {
    const r0 = orbitRadius(seg);
    const r1 = orbitRadius(seg + 1);
    const a0 = planetAngle(seg);
    const a1 = planetAngle(seg + 1);
    for (let k = seg === from ? 0 : 1; k <= samples; k++) {
      const t = k / samples;
      pts.push(polar(r0 + (r1 - r0) * t, a0 + (a1 - a0) * t));
    }
  }
  return pts.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" ");
}

/** Curved shuttle lane between two planets, bulging away from the star. */
export function shuttlePath(a: number, b: number): { d: string; mid: Point } {
  const pa = planetPosition(a);
  const pb = planetPosition(b);
  const c = polar((orbitRadius(a) + orbitRadius(b)) / 2 + 95, (planetAngle(a) + planetAngle(b)) / 2);
  const mx = 0.25 * pa.x + 0.5 * c.x + 0.25 * pb.x;
  const my = 0.25 * pa.y + 0.5 * c.y + 0.25 * pb.y;
  // the round tag sits a little outside the lane so the ship's moons never cover it
  const len = Math.hypot(mx, my) || 1;
  const mid = { x: round2(mx + (mx / len) * 34), y: round2(my + (my / len) * 34) };
  return { d: `M ${pa.x} ${pa.y} Q ${c.x} ${c.y} ${pb.x} ${pb.y}`, mid };
}

// ---------- asteroid belt (findings) ----------

/** Findings orbit between the fix and cross-review planets, where they are worked on. */
export function beltRadius(): number {
  return (orbitRadius(stepIndex("fix")) + orbitRadius(stepIndex("crossReview"))) / 2;
}

/** FNV-1a 32-bit hash, stable across runs. */
export function hashString(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** Deterministic pseudo-random sequence in [0, 1). */
export function seededRandom(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const ASTEROID_SIZE: Record<Severity, number> = { critical: 11.5, high: 10, medium: 8.5, low: 7 };

/**
 * The belt sits radially between the Review and Fix orbits, on the far side of
 * the map (lower-left), away from the planets, the ship and the shuttle lane.
 */
export const BELT_START_DEG = 104;
export const BELT_END_DEG = 204;

export function beltArcPath(): string {
  return arcPath(beltRadius(), BELT_START_DEG - 6, BELT_END_DEG + 6);
}

export interface AsteroidPlacement {
  x: number;
  y: number;
  size: number;
  rotation: number;
}

/** Spread findings along the belt. */
export function asteroidLayout(items: { fingerprint: string; severity: Severity }[]): Map<string, AsteroidPlacement> {
  const out = new Map<string, AsteroidPlacement>();
  const start = BELT_START_DEG;
  const end = BELT_END_DEG;
  const n = items.length;
  items.forEach((item, i) => {
    const rnd = seededRandom(hashString(item.fingerprint));
    const angle = start + ((i + 0.5) * (end - start)) / Math.max(n, 1) + (rnd() - 0.5) * 6;
    const r = beltRadius() + (rnd() - 0.5) * 14;
    const p = polar(r, angle);
    out.set(item.fingerprint, { ...p, size: ASTEROID_SIZE[item.severity], rotation: Math.round(rnd() * 360) });
  });
  return out;
}

/** Irregular rock outline centred on (0, 0). */
export function asteroidShape(seed: string, size: number, vertices = 7): string {
  const rnd = seededRandom(hashString(seed) ^ 0x9e3779b9);
  const pts: string[] = [];
  for (let i = 0; i < vertices; i++) {
    const a = (i / vertices) * 360 + (rnd() - 0.5) * 22;
    const r = size * (0.52 + rnd() * 0.48);
    const p = polar(r, a);
    pts.push(`${p.x},${p.y}`);
  }
  return pts.join(" ");
}

// ---------- ship atmosphere (context) ----------

export const ATMO_BASE = 12;
export const ATMO_SPAN = 26;

/** Radius of the atmosphere; reaches the dashed handoff ring at 100%. */
export function atmosphereRadius(contextTokens: number, handoffAt: number): number {
  if (handoffAt <= 0) return ATMO_BASE;
  const ratio = Math.min(Math.max(contextTokens / handoffAt, 0), 1.2);
  return round2(ATMO_BASE + ATMO_SPAN * ratio);
}

export function handoffRingRadius(): number {
  return ATMO_BASE + ATMO_SPAN;
}

// ---------- fuel ring (quota) ----------

export interface FuelArc {
  key: string;
  provider: QuotaSnapshot["provider"];
  kind: QuotaWindow["kind"];
  track: string;
  fill: string | null;
  remainingPct: number;
  /** false when the snapshot has no data for this window yet */
  known: boolean;
  label: Point;
  labelAnchor: "start" | "end";
}

// The fuel gauge spans the top half of the map, where no planet labels reach.
const FUEL_SLOTS: { provider: QuotaSnapshot["provider"]; kind: QuotaWindow["kind"]; center: number }[] = [
  { provider: "claude", kind: "fiveHour", center: -152 },
  { provider: "claude", kind: "sevenDay", center: -112 },
  { provider: "codex", kind: "fiveHour", center: -68 },
  { provider: "codex", kind: "sevenDay", center: -28 },
];

export function fuelArcs(quota: QuotaSnapshot[], now: number): FuelArc[] {
  return FUEL_SLOTS.map((slot) => {
    const win = quota.find((q) => q.provider === slot.provider)?.windows.find((w) => w.kind === slot.kind);
    const start = slot.center - FUEL_SPAN_DEG / 2;
    const end = slot.center + FUEL_SPAN_DEG / 2;
    const remaining = remainingPct(win, now);
    const fillEnd = start + (FUEL_SPAN_DEG * remaining) / 100;
    const label = polar(FUEL_R + 20, slot.center);
    return {
      key: `${slot.provider}-${slot.kind}`,
      provider: slot.provider,
      kind: slot.kind,
      track: arcPath(FUEL_R, start, end),
      fill: remaining > 0.5 ? arcPath(FUEL_R, start, fillEnd) : null,
      remainingPct: remaining,
      known: win !== undefined,
      label,
      labelAnchor: label.x >= 0 ? "start" : "end",
    };
  });
}

// ---------- nebulae, distant stars, starfield ----------

export interface Placement extends Point {
  /** 1 = full size; smaller when many items share the map */
  scale: number;
}

// First ring clears the empty centre (protostar + hint pill).
const NEBULA_FIRST_RING = 210;
const NEBULA_RING_GAP = 135;
/** Arc length one nebula needs at full size (cloud + two label lines + breathing room). */
const NEBULA_SLOT = 175;
const NEBULA_MIN_SCALE = 0.55;
/** Up to this many tasks keep full size. */
const NEBULA_FULL_SIZE = 6;

/**
 * Inbox tasks as nebulae on as many rings as needed: each ring takes as many as fit its
 * circumference, rings are staggered, and everything shrinks a little per extra task so a
 * long inbox stays readable instead of piling up on one circle.
 */
export function nebulaLayout(n: number): Placement[] {
  const scale =
    n <= NEBULA_FULL_SIZE ? 1 : round2(Math.max(NEBULA_MIN_SCALE, 1 - (n - NEBULA_FULL_SIZE) * 0.03));
  const slot = NEBULA_SLOT * scale;
  const out: Placement[] = [];
  for (let ring = 0, left = n; left > 0; ring++) {
    const r = NEBULA_FIRST_RING + ring * NEBULA_RING_GAP * scale;
    const capacity = Math.max(1, Math.floor((2 * Math.PI * r) / slot));
    const count = Math.min(capacity, left);
    const start = -65 + ring * (180 / capacity);
    for (let k = 0; k < count; k++) out.push({ ...polar(r, start + (k * 360) / count), scale });
    left -= count;
  }
  return out;
}

/** Hand-placed corners for the usual few outside sessions. */
const DISTANT_SLOTS: Point[] = [
  { x: -418, y: -330 },
  { x: 414, y: -334 },
  { x: 416, y: 326 },
  { x: -412, y: 330 },
  { x: 440, y: 40 },
  { x: 0, y: 384 },
];
// More sessions go on a U along the sides and bottom; the top half belongs to the fuel gauge
// and its labels, so only its two far corners are used.
const DISTANT_SIDE_X = 440;
const DISTANT_SIDE_TOP_Y = -120;
const DISTANT_BOTTOM_Y = 348;

function alongU(d: number): Point {
  const side = DISTANT_BOTTOM_Y - DISTANT_SIDE_TOP_Y;
  const bottom = 2 * DISTANT_SIDE_X;
  if (d <= side) return { x: -DISTANT_SIDE_X, y: DISTANT_SIDE_TOP_Y + d };
  if (d <= side + bottom) return { x: -DISTANT_SIDE_X + (d - side), y: DISTANT_BOTTOM_Y };
  return { x: DISTANT_SIDE_X, y: DISTANT_BOTTOM_Y - (d - side - bottom) };
}

/**
 * Outside sessions at the edge of the map: hand-placed spots for a few, otherwise the two top
 * corners plus an even spread along the sides and bottom, so every session gets a star.
 */
export function distantPositions(n: number): Point[] {
  if (n <= DISTANT_SLOTS.length) return DISTANT_SLOTS.slice(0, n);
  const corners = DISTANT_SLOTS.slice(0, 2);
  const rest = n - corners.length;
  const length = 2 * (DISTANT_BOTTOM_Y - DISTANT_SIDE_TOP_Y) + 2 * DISTANT_SIDE_X;
  const spread = Array.from({ length: rest }, (_, i) => {
    const p = alongU(rest === 1 ? length / 2 : (i * length) / (rest - 1));
    return { x: round2(p.x), y: round2(p.y) };
  });
  return [...corners, ...spread];
}

export interface Dot {
  x: number;
  y: number;
  r: number;
  twinkle: boolean;
}

/** Background dots over an area wider than the view, so wide windows stay filled. */
export function starfield(count: number, seed = 7): Dot[] {
  const rnd = seededRandom(seed);
  const spreadX = VIEW.w * 1.8;
  const spreadY = VIEW.h * 1.4;
  return Array.from({ length: count }, () => ({
    x: Math.round(-spreadX / 2 + rnd() * spreadX),
    y: Math.round(-spreadY / 2 + rnd() * spreadY),
    r: round2(0.35 + rnd() * 1.1),
    // few twinklers: every animated dot repaints the whole map each frame
    twinkle: rnd() < 0.07,
  }));
}

export function clampZoom(z: number): number {
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(z * 100) / 100));
}
