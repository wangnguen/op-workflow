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

/**
 * How far planets have moved along their orbits, in degrees. A number moves every planet by the
 * same angle; a function gives each step (1-based, may be fractional for the ship in flight) its
 * own offset. Ship, gate, trail and belt follow the planets they belong to.
 */
export type Spin = number | ((index: number) => number);

function offsetOf(spin: Spin, index: number): number {
  if (typeof spin === "number") return spin;
  const lo = Math.floor(index);
  const hi = Math.ceil(index);
  if (lo === hi) return spin(lo);
  const t = index - lo;
  return spin(lo) * (1 - t) + spin(hi) * t;
}

/**
 * Planets sway together along their orbits (a slow common swing) and each adds a small wobble of
 * its own. Neighbours keep their spacing, so labels never meet, and the fuel gauge stays clear.
 */
const SWAY_DEG = 7;
const SWAY_PERIOD_S = 48;
const WOBBLE_DEG = 3;
const WOBBLE_PERIOD_S = 21;
const WOBBLE_PERIOD_STEP_S = 4;
export const DRIFT_DEG = SWAY_DEG + WOBBLE_DEG;

/** Offset of every planet `t` seconds into the motion (degrees, at most DRIFT_DEG). */
export function planetDrift(t: number): (index: number) => number {
  const sway = SWAY_DEG * Math.sin((2 * Math.PI * t) / SWAY_PERIOD_S);
  return (index) =>
    sway +
    WOBBLE_DEG * Math.sin((2 * Math.PI * t) / (WOBBLE_PERIOD_S + index * WOBBLE_PERIOD_STEP_S) + index * 1.7);
}

/** Planets form an outward spiral: the journey reads clockwise from upper-left. */
export function planetAngle(index: number, rot: Spin = 0): number {
  return JOURNEY_START_DEG + (index - 1) * JOURNEY_STEP_DEG + offsetOf(rot, index);
}

export function planetPosition(index: number, rot: Spin = 0): Point {
  return polar(orbitRadius(index), planetAngle(index, rot));
}

const SHIP_LEAD = 62;
const GATE_LAG = 50;
const arcDeg = (length: number, r: number) => (length / r) * (180 / Math.PI);

/** Ship rides its step's orbit a fixed arc length ahead of the planet (toward open space). */
export function shipAngle(index: number, rot: Spin = 0): number {
  return planetAngle(index, rot) + arcDeg(SHIP_LEAD, orbitRadius(index));
}

export function shipPosition(index: number, rot: Spin = 0): Point {
  return polar(orbitRadius(index), shipAngle(index, rot));
}

/** Heading of the ship (tangent of the clockwise orbit), in degrees. */
export function shipHeading(index: number, rot: Spin = 0): number {
  return shipAngle(index, rot) + 90;
}

/** Axis-aligned box in map units (for keeping labels apart). */
export interface Box {
  l: number;
  r: number;
  t: number;
  b: number;
}

export const boxesHit = (a: Box, b: Box) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b;

/** Widest ship label ("Claude + Codex · 100%") and planet label we plan room for. */
export const SHIP_LABEL_W = 138;
const PLANET_LABEL_W = 120;
/** The task name under the star. */
export const STAR_LABEL_BOX: Box = { l: -62, r: 62, t: 32, b: 68 };
/** The "+N comment" badge up and right of the star (see Meteor). */
export const COMMENT_BADGE_BOX: Box = { l: STAR_R + 10, r: STAR_R + 94, t: -STAR_R - 24, b: -STAR_R - 4 };

/** Box of planet `index`'s two-line label (conservative width). */
export function planetLabelBox(index: number, rot: Spin = 0, width = PLANET_LABEL_W): Box {
  const c = labelPosition(index, rot, width);
  return { l: c.x - width / 2, r: c.x + width / 2, t: c.y - 13, b: c.y + 15 };
}

/** Moments of the drift used to check that a label spot stays clear while planets move. */
let driftSamples: Spin[] | null = null;
const samples = () => (driftSamples ??= Array.from({ length: 160 }, (_, k) => planetDrift(k * 2.5)));

/** Extra room around a label being placed, so it stays clear between the sampled moments. */
const CLEARANCE = 5;

/**
 * True when the box `at(rot)` (plus CLEARANCE) never meets a planet label (except planet `own`'s)
 * or the star's name, at any sampled moment of the drift.
 */
function clearWhileDrifting(at: (rot: Spin) => Box, own?: number): boolean {
  return samples().every((rot) => {
    const b0 = at(rot);
    const box = { l: b0.l - CLEARANCE, r: b0.r + CLEARANCE, t: b0.t - CLEARANCE, b: b0.b + CLEARANCE };
    if (boxesHit(box, STAR_LABEL_BOX) || boxesHit(box, COMMENT_BADGE_BOX)) return false;
    for (let i = 1; i <= STEPS.length; i++) {
      if (own !== undefined && i === Math.round(own)) continue;
      if (boxesHit(box, planetLabelBox(i, rot))) return false;
    }
    return true;
  });
}

type ShipLabel = Point & { anchor: "start" | "end" | "middle" };

function shipLabelAt(index: number, ringRadius: number, rot: Spin, turn: number, gap = 12): ShipLabel {
  const p = polar(ringRadius + gap, shipAngle(index, rot) + turn);
  const anchor = Math.abs(p.x) < 12 ? "middle" : p.x > 0 ? "start" : "end";
  return { x: p.x, y: p.y + (p.y > 0 ? 8 : 0), anchor };
}

/** Box of a ship label placed at `o` (relative to the ship at `at`). */
export function shipLabelBox(at: Point, o: ShipLabel, width = SHIP_LABEL_W): Box {
  const x = at.x + o.x;
  const l = o.anchor === "start" ? x : o.anchor === "end" ? x - width : x - width / 2;
  return { l, r: l + width, t: at.y + o.y - 12, b: at.y + o.y + 4 };
}

/** Sides tried for the ship label, relative to the ship's angle: toward the star first, then away,
 * behind, ahead and the diagonals; a little farther out when nothing close is free. */
const SHIP_LABEL_TURNS = [180, 0, -90, 90, 135, -135, 45, -45];
const SHIP_LABEL_GAPS = [12, 26];
const shipTurnCache = new Map<string, { turn: number; gap: number }>();

/**
 * Which side the ship label goes on for a ship on orbit `index`: the first side that clears
 * every planet label and the star's name in the resting layout. Fixed per orbit, so the label
 * follows the drifting ship smoothly and never flips sides.
 */
function shipLabelSpot(index: number, ringRadius: number): { turn: number; gap: number } {
  const key = `${index}|${ringRadius}`;
  const cached = shipTurnCache.get(key);
  if (cached !== undefined) return cached;
  let spot = { turn: 180, gap: 12 };
  search: for (const gap of SHIP_LABEL_GAPS) {
    for (const turn of SHIP_LABEL_TURNS) {
      const clear = clearWhileDrifting(
        (rot) => shipLabelBox(shipPosition(index, rot), shipLabelAt(index, ringRadius, rot, turn, gap)),
        index,
      );
      if (clear) {
        spot = { turn, gap };
        break search;
      }
    }
  }
  shipTurnCache.set(key, spot);
  return spot;
}

/** Ship label just outside the handoff ring, on the side that keeps clear of other labels. */
export function shipLabelOffset(index: number, ringRadius: number, rot: Spin = 0): ShipLabel {
  // a ship in flight between orbits uses the side of the orbit it is closest to
  const spot = shipLabelSpot(Math.round(index), ringRadius);
  return shipLabelAt(index, ringRadius, rot, spot.turn, spot.gap);
}

/** Gate station sits on the orbit just behind the planet. */
export function gatePosition(index: number, rot: Spin = 0): Point {
  return polar(orbitRadius(index), planetAngle(index, rot) - arcDeg(GATE_LAG, orbitRadius(index)));
}

/** Widest gate label we plan room for ("Quyết finding" and the like). */
export const GATE_LABEL_W = 112;

type GateLabel = Point & { anchor: "start" | "end" | "middle" };

/** Direction (degrees) from the planet to its gate: "behind" the planet on the orbit. */
function gateBehindDeg(index: number, rot: Spin): number {
  const g = gatePosition(index, rot);
  const p = planetPosition(index, rot);
  return (Math.atan2(g.y - p.y, g.x - p.x) * 180) / Math.PI;
}

function gateLabelAt(index: number, rot: Spin, turn: number, dist = 24): GateLabel {
  const deg = gateBehindDeg(index, rot) + turn;
  const a = (deg * Math.PI) / 180;
  const dx = Math.cos(a);
  const dy = Math.sin(a);
  return {
    x: round2(dx * dist),
    y: round2(dy * dist + 4),
    anchor: Math.abs(dx) < 0.35 ? "middle" : dx > 0 ? "start" : "end",
  };
}

/** Box of a gate label placed at `o` (relative to the gate at `at`). */
export function gateLabelBox(at: Point, o: GateLabel, width = GATE_LABEL_W): Box {
  const x = at.x + o.x;
  const l = o.anchor === "start" ? x : o.anchor === "end" ? x - width : x - width / 2;
  return { l, r: l + width, t: at.y + o.y - 12, b: at.y + o.y + 4 };
}

/** Behind the planet first, then turning away from it step by step. */
const GATE_TURNS = [0, -30, 30, -60, 60, -90, 90, -120, 120, -150, 150, 180];
/** Close to the station first; a little farther when every close spot is taken. */
const GATE_DISTS = [24, 38, 52];
const gateTurnCache = new Map<number, { turn: number; dist: number }>();

/** The spot nearest "behind" whose label clears every resting label (fixed per step). */
function gateLabelSpot(index: number): { turn: number; dist: number } {
  const cached = gateTurnCache.get(index);
  if (cached !== undefined) return cached;
  let spot = { turn: 0, dist: 24 };
  search: for (const dist of GATE_DISTS) {
    for (const turn of GATE_TURNS) {
      const clear = clearWhileDrifting(
        (rot) => gateLabelBox(gatePosition(index, rot), gateLabelAt(index, rot, turn, dist)),
      );
      if (clear) {
        spot = { turn, dist };
        break search;
      }
    }
  }
  gateTurnCache.set(index, spot);
  return spot;
}

/** Gate label offset (relative to the gate), on a side fixed per step that keeps clear of labels. */
export function gateLabelOffset(index: number, rot: Spin = 0): GateLabel {
  const spot = gateLabelSpot(index);
  return gateLabelAt(index, rot, spot.turn, spot.dist);
}

export interface LabelLayout extends Point {
  anchor: "start" | "end" | "middle";
  /** baseline offsets (from the anchor point) of the two text lines */
  mainDy: number;
  subDy: number;
}

/** Height of the two-line planet label block. */
const LABEL_H = 28;

/**
 * Two-line planet label, centred on a point pushed outward from the star just far enough that
 * the label box (`width` wide) clears the planet. It changes continuously with the angle, so a
 * turning system never makes a label jump from one side to the other.
 */
export function labelPosition(index: number, rot: Spin = 0, width = 96): LabelLayout {
  const a = (planetAngle(index, rot) * Math.PI) / 180;
  const ux = Math.cos(a);
  const uy = Math.sin(a);
  const gap = PLANET_R + 8;
  const push = gap + Math.abs(ux) * (width / 2) + Math.abs(uy) * (LABEL_H / 2);
  const r = orbitRadius(index);
  return {
    x: round2((r + push) * ux),
    y: round2((r + push) * uy),
    anchor: "middle",
    mainDy: -2,
    subDy: 12,
  };
}

/** Rough width of a label in user units (Inter at the planet label sizes). */
export function labelWidth(main: string, sub: string): number {
  return Math.max(main.length * 7.2, sub.length * 6.2);
}

export function arcPath(r: number, startDeg: number, endDeg: number): string {
  const s = polar(r, startDeg);
  const e = polar(r, endDeg);
  const large = Math.abs(endDeg - startDeg) > 180 ? 1 : 0;
  const sweep = endDeg >= startDeg ? 1 : 0;
  return `M ${s.x} ${s.y} A ${r} ${r} 0 ${large} ${sweep} ${e.x} ${e.y}`;
}

/** Spiral flight path from planet `from` to planet `to` (indices, from < to). */
export function spiralPath(from: number, to: number, samples = 14, rot: Spin = 0): string {
  if (to <= from) return "";
  const pts: Point[] = [];
  for (let seg = from; seg < to; seg++) {
    const r0 = orbitRadius(seg);
    const r1 = orbitRadius(seg + 1);
    const a0 = planetAngle(seg, rot);
    const a1 = planetAngle(seg + 1, rot);
    for (let k = seg === from ? 0 : 1; k <= samples; k++) {
      const t = k / samples;
      pts.push(polar(r0 + (r1 - r0) * t, a0 + (a1 - a0) * t));
    }
  }
  return pts.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" ");
}

/** Curved shuttle lane between two planets, bulging away from the star. */
export function shuttlePath(a: number, b: number, rot: Spin = 0): { d: string; mid: Point } {
  const pa = planetPosition(a, rot);
  const pb = planetPosition(b, rot);
  const c = polar((orbitRadius(a) + orbitRadius(b)) / 2 + 95, (planetAngle(a, rot) + planetAngle(b, rot)) / 2);
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

/** The belt lies between the review and fix orbits and drifts with them. */
const beltOffset = (rot: Spin) => offsetOf(rot, (stepIndex("crossReview") + stepIndex("fix")) / 2);

export function beltArcPath(rot: Spin = 0): string {
  const o = beltOffset(rot);
  return arcPath(beltRadius(), BELT_START_DEG - 6 + o, BELT_END_DEG + 6 + o);
}

export interface AsteroidPlacement {
  x: number;
  y: number;
  size: number;
  rotation: number;
}

/** Spread findings along the belt. */
export function asteroidLayout(
  items: { fingerprint: string; severity: Severity }[],
  rot: Spin = 0,
): Map<string, AsteroidPlacement> {
  const out = new Map<string, AsteroidPlacement>();
  const o = beltOffset(rot);
  const start = BELT_START_DEG + o;
  const end = BELT_END_DEG + o;
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
// The top corners sit a little lower than the very edge: the status chip (left) and the legend
// button (right) float over the top band of the map.
const DISTANT_SLOTS: Point[] = [
  { x: -418, y: -262 },
  { x: 414, y: -266 },
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
