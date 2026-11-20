import type { Spin } from "./geometry";
import { useRef, useState } from "react";
import type { PointerEvent, WheelEvent } from "react";
import type { OrbitSnapshot, TodayStats } from "../../ipc/types";
import { VIEW, clampZoom, stepIndex, type Point } from "./geometry";
import { Asteroids } from "./scene/Asteroids";
import { DistantStars } from "./scene/DistantStars";
import { FuelRing } from "./scene/FuelRing";
import { GateStation } from "./scene/GateStation";
import { Meteor } from "./scene/Meteor";
import { Nebulae } from "./scene/Nebulae";
import { Orbits, type CompactionMarker } from "./scene/Orbits";
import { Planets } from "./scene/Planets";
import { SceneDefs } from "./scene/SceneDefs";
import { Ship } from "./scene/Ship";
import { Star } from "./scene/Star";
import { Starfield } from "./scene/Starfield";
import type { Selection } from "./selection";
import type { LayoutOverrides } from "./useLayout";

export interface ViewState {
  zoom: number;
  x: number;
  y: number;
}

export const HOME_VIEW: ViewState = { zoom: 1, x: 0, y: 0 };

/** What the sun in the centre shows while no task is in progress. */
export interface MapSummary {
  /** sessions with a turn running (the app's and those opened outside it) */
  running: number;
  /** sessions waiting for an answer */
  waiting: number;
  today?: TodayStats;
}

interface Props {
  snapshot: OrbitSnapshot;
  selection: Selection | null;
  onSelect: (s: Selection) => void;
  /** a double click on an item: do what its panel's main button does */
  onOpen?: (s: Selection) => void;
  view: ViewState;
  onView: (v: ViewState) => void;
  markers: CompactionMarker[];
  now: number;
  /** positions the user dragged items to */
  layout: LayoutOverrides;
  /** `persist` is true once, when the drag ends */
  onMoveItem: (key: string, p: Point, persist: boolean) => void;
  /** degrees the planetary system has turned (see useSpin) */
  rot?: Spin;
  summary?: MapSummary;
}

/** Starts dragging one map item (nebula, far star) from where it is drawn now. */
export type ItemDragStart = (key: string, from: Point) => (e: PointerEvent<SVGGElement>) => void;

const DRAG_THRESHOLD_PX = 4;
/** A double click opens the item the click just before it picked, if it came this soon. */
const OPEN_AFTER_PICK_MS = 600;

export function OrbitScene({ snapshot, selection, onSelect: select, onOpen, view, onView, markers, now, layout, onMoveItem, rot = 0, summary }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  // the item the last click picked, so the double click that follows opens that very item
  const lastPick = useRef<{ sel: Selection; at: number } | null>(null);
  const onSelect = (s: Selection) => {
    lastPick.current = { sel: s, at: performance.now() };
    select(s);
  };
  const drag = useRef<{ px: number; py: number; vx: number; vy: number; moved: boolean } | null>(null);
  const justDragged = useRef(false);
  const [dragging, setDragging] = useState(false);
  const [itemDragging, setItemDragging] = useState(false);
  const active = snapshot.active;

  const unitsPerPx = () => {
    const rect = svgRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0 || rect.height === 0) return 1;
    return Math.max(VIEW.w / rect.width, VIEW.h / rect.height);
  };

  const suppressNextClick = () => {
    justDragged.current = true;
    window.setTimeout(() => (justDragged.current = false), 0);
  };

  const startItemDrag: ItemDragStart = (key, from) => (e) => {
    if (e.button !== 0) return;
    // the map itself must not pan while an item moves
    e.stopPropagation();
    const start = { x: e.clientX, y: e.clientY };
    const k = unitsPerPx() / view.zoom;
    let moved = false;
    let last = from;
    const onMove = (ev: globalThis.PointerEvent) => {
      const dx = ev.clientX - start.x;
      const dy = ev.clientY - start.y;
      if (!moved && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      if (!moved) {
        moved = true;
        setItemDragging(true);
      }
      last = { x: from.x + dx * k, y: from.y + dy * k };
      onMoveItem(key, last, false);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      setItemDragging(false);
      if (moved) {
        onMoveItem(key, last, true);
        suppressNextClick();
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  };

  const onPointerDown = (e: PointerEvent<SVGSVGElement>) => {
    if (e.button !== 0) return;
    drag.current = { px: e.clientX, py: e.clientY, vx: view.x, vy: view.y, moved: false };
  };
  const onPointerMove = (e: PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.px;
    const dy = e.clientY - d.py;
    if (!d.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
    if (!d.moved) {
      d.moved = true;
      setDragging(true);
      e.currentTarget.setPointerCapture(e.pointerId);
    }
    const k = unitsPerPx();
    onView({ ...view, x: d.vx + dx * k, y: d.vy + dy * k });
  };
  const endDrag = () => {
    // the click that follows this pointerup must not select anything
    if (drag.current?.moved) suppressNextClick();
    drag.current = null;
    setDragging(false);
  };
  const onWheel = (e: WheelEvent<SVGSVGElement>) => {
    onView({ ...view, zoom: clampZoom(view.zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1)) });
  };

  const current = active
    ? (active.steps.find((s) => s.state === "running" || s.state === "waitingUser")?.index ??
      active.steps.filter((s) => s.state === "done").length)
    : 0;
  const agentOrbit = active?.agents[0] ? stepIndex(active.agents[0].step) : current;

  return (
    <svg
      ref={svgRef}
      className={`orbit-svg${dragging || itemDragging ? " is-dragging" : ""}`}
      viewBox={`${VIEW.x} ${VIEW.y} ${VIEW.w} ${VIEW.h}`}
      preserveAspectRatio="xMidYMid meet"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onWheel={onWheel}
      onDoubleClick={() => {
        const p = lastPick.current;
        if (p && performance.now() - p.at < OPEN_AFTER_PICK_MS) onOpen?.(p.sel);
      }}
      onClickCapture={(e) => {
        if (justDragged.current) e.stopPropagation();
      }}
    >
      <SceneDefs />
      <Starfield />
      <g
        className={`orbit-world${dragging || itemDragging ? "" : " is-smooth"}`}
        style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.zoom})` }}
      >
        <FuelRing quota={snapshot.quota} now={now} />
        {active ? (
          <>
            <Orbits steps={active.steps} current={current} markers={markers} rot={rot} />
            <Asteroids
              rot={rot}
              findings={active.findings}
              selectedFingerprint={selection?.kind === "finding" ? selection.fingerprint : undefined}
              onSelect={(fingerprint) => onSelect({ kind: "finding", fingerprint })}
            />
            <Planets
              rot={rot}
              steps={active.steps}
              selectedStep={selection?.kind === "step" ? selection.step : undefined}
              onSelect={(step) => onSelect({ kind: "step", step })}
            />
            {active.gate && current > 0 && (
              <GateStation
                rot={rot}
                gate={active.gate}
                stepIndex={current}
                selected={selection?.kind === "gate"}
                onSelect={() => onSelect({ kind: "gate" })}
              />
            )}
            <Star task={active} selected={selection?.kind === "star"} onSelect={() => onSelect({ kind: "star" })} />
            {active.agents.map((agent) => (
              <Ship
                key={agent.id}
                rot={rot}
                agent={agent}
                orbit={agentOrbit}
                selectedId={selection?.kind === "agent" ? selection.id : undefined}
                onSelect={(id) => onSelect({ kind: "agent", id })}
              />
            ))}
            <Meteor count={active.newComments} onSelect={() => onSelect({ kind: "star" })} />
          </>
        ) : (
          <Nebulae
            inbox={snapshot.inbox}
            summary={summary}
            selectedTaskId={selection?.kind === "nebula" ? selection.taskId : undefined}
            onSelect={(taskId) => onSelect({ kind: "nebula", taskId })}
            layout={layout}
            onDragStart={startItemDrag}
          />
        )}
        <DistantStars
          sessions={snapshot.distant}
          selectedId={selection?.kind === "distant" ? selection.id : undefined}
          onSelect={(id) => onSelect({ kind: "distant", id })}
          layout={layout}
          onDragStart={startItemDrag}
        />
      </g>
    </svg>
  );
}
