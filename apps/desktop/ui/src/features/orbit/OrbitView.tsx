import { useMemo, useRef, useState, type CSSProperties } from "react";
import { agentLabel, gateLabel, sessionStatusLabel, stepLabel, stepStateLabel, vi } from "../../i18n/vi";
import type { OrbitSnapshot } from "../../ipc/types";
import { useDismiss } from "../../shared/hooks";
import { STEP_COUNT } from "../../shared/steps";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { SidePanel } from "../../shared/ui/SidePanel";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { DetailPanel } from "./DetailPanel";
import { Legend } from "./Legend";
import { HOME_VIEW, OrbitScene, type ViewState } from "./OrbitScene";
import { focusSystem, systemsOf } from "./focus";
import { primaryAction, type Go } from "./primary";
import { VIEW, clampZoom, planetDrift, topLeftFuelLabelX } from "./geometry";
import { resolveSelection, type Selection } from "./selection";
import { useCompactionMarkers } from "./useCompactionMarkers";
import { useLayout } from "./useLayout";
import { useMotionClock } from "./motion";
import "./orbit.css";
import type { TaskTab } from "../task/taskTabs";

interface Props {
  snapshot: OrbitSnapshot;
  now: number;
  /** task in the centre; when given, the parent owns it (Phiên centres a task it just started) */
  focusId?: string | null;
  onFocus?: (taskId: string | null) => void;
  /** "Bắt đầu làm việc" on an Inbox task picked on the map */
  onStartTask?: (taskId: string) => void;
  /** open the task window of a task in progress */
  onOpenTask?: (taskId: string, tab?: TaskTab) => void;
  /** open an Inbox task on OpenProject */
  onOpenLink?: (wpId: number) => void;
  /** the map is hidden (Phiên shows Bảng): stop the motion, keep the angle */
  paused?: boolean;
  /** the main button of the picked item, or a double click on it */
  onGo?: (go: Go) => void;
  /** task ids whose session waits for an answer: their gate leads to the question */
  waiting?: ReadonlySet<string>;
}

const NONE: ReadonlySet<string> = new Set();

/** Where the map's top-left status line has to stop, for orbit.css. */
const STATUS_CLEAR = {
  "--status-clear": topLeftFuelLabelX(),
  "--view-w": VIEW.w,
  "--view-h": VIEW.h,
} as CSSProperties;

function statusLine(s: OrbitSnapshot): string {
  const a = s.active;
  if (!a) return vi.orbit.noActive;
  const live = a.steps.find((x) => x.state === "running" || x.state === "waitingUser");
  const parts = [a.taskId];
  if (live) parts.push(`${live.index}/${STEP_COUNT} ${stepLabel[live.kind]}${live.round && live.round > 1 ? ` ${vi.orbit.round(live.round)}` : ""}`);
  else {
    const last = [...a.steps].reverse().find((x) => x.state === "done");
    if (last) parts.push(`${last.index}/${STEP_COUNT} ${stepLabel[last.kind]} · ${stepStateLabel.done}`);
  }
  if (a.gate) parts.push(gateLabel[a.gate.kind]);
  const agent = a.agents[0];
  const child = agent?.children.find((c) => c.status === "running");
  const runningText = sessionStatusLabel.running.toLowerCase();
  if (child) parts.push(`${agentLabel[child.kind]} ${runningText}`);
  else if (agent?.status === "running") parts.push(`${agentLabel[agent.kind]} ${runningText}`);
  return parts.join(" · ");
}

export function OrbitView({
  snapshot: live,
  now,
  focusId: focusProp,
  onFocus,
  onStartTask,
  onOpenTask,
  onOpenLink,
  paused = false,
  onGo,
  waiting = NONE,
}: Props) {
  const [picked, setPicked] = useState<Selection | null>(null);
  // several tasks can be in progress; the picked one sits in the centre
  const [ownFocus, setOwnFocus] = useState<string | null>(null);
  const focusId = focusProp !== undefined ? focusProp : ownFocus;
  const setFocusId = onFocus ?? setOwnFocus;
  const snapshot = useMemo(() => focusSystem(live, focusId), [live, focusId]);
  const systems = systemsOf(live);
  const [view, setView] = useState<ViewState>(HOME_VIEW);
  const [still, setStill] = useState(false);
  const [legendOpen, setLegendOpen] = useState(false);
  const legendRef = useRef<HTMLDivElement>(null);
  useDismiss(legendRef, legendOpen, () => setLegendOpen(false));
  const markers = useCompactionMarkers(snapshot.active);
  const layout = useLayout();
  const selection = resolveSelection(picked, snapshot);
  const primary = primaryAction(selection, snapshot, waiting);
  // planets drift on their orbits; reduced motion, a hidden map or no task stops them in place
  const seconds = useMotionClock(!still && !paused && snapshot.active !== undefined);
  const rot = useMemo(() => planetDrift(seconds), [seconds]);

  return (
    <div className={`orbit-view${still ? " is-still" : ""}`}>
      <div className="orbit-stage">
        <OrbitScene
          snapshot={snapshot}
          selection={selection}
          onSelect={setPicked}
          onOpen={(sel) => {
            const go = primaryAction(sel, snapshot, waiting)?.go;
            if (go && onGo && (go.to !== "start" || onStartTask)) onGo(go);
          }}
          view={view}
          onView={setView}
          markers={markers}
          now={now}
          layout={layout.overrides}
          onMoveItem={layout.move}
          rot={rot}
        />

        {/* wraps before the "Claude Tuần" gauge label (map units → px in orbit.css) */}
        <div
          className={`orbit-status${snapshot.active?.gate ? " is-waiting" : snapshot.active ? "" : " is-idle"}`}
          style={STATUS_CLEAR}
          title={snapshot.active?.title}
        >
          <span className="orbit-status-dot" />
          {statusLine(snapshot)}
        </div>

        {systems.length > 1 && snapshot.active && (
          <div className="orbit-systems">
            <span className="orbit-systems-label">{vi.orbit.systems}</span>
            <SegmentedControl
              label={vi.orbit.systems}
              value={snapshot.active.taskId}
              onChange={(id) => {
                setFocusId(id);
                setPicked(null);
              }}
              options={systems.map((s) => {
                const step = s.steps.find((x) => x.state === "running" || x.state === "waitingUser");
                const last = [...s.steps].reverse().find((x) => x.state === "done");
                const shown = step ?? last;
                return {
                  value: s.taskId,
                  label: s.taskId,
                  icon: <StatusIcon state={s.gate ? "waitingUser" : (step?.state ?? "done")} size={12} />,
                  title: `${s.taskId} · ${s.title}${shown ? `
${shown.index}/${STEP_COUNT} ${stepLabel[shown.kind]}` : ""}
${vi.orbit.systemPick}`,
                };
              })}
            />
          </div>
        )}

        <div className="orbit-legend" ref={legendRef}>
          <button className="btn btn--sm" aria-expanded={legendOpen} onClick={() => setLegendOpen((v) => !v)}>
            {vi.orbit.legend} ▾
          </button>
          {legendOpen && <Legend />}
        </div>

        <div className="orbit-controls" role="toolbar">
          <button className="btn btn--icon" title={vi.orbit.zoomIn} aria-label={vi.orbit.zoomIn}
            onClick={() => setView((v) => ({ ...v, zoom: clampZoom(v.zoom * 1.2) }))}>
            +
          </button>
          <button className="btn btn--icon" title={vi.orbit.zoomOut} aria-label={vi.orbit.zoomOut}
            onClick={() => setView((v) => ({ ...v, zoom: clampZoom(v.zoom / 1.2) }))}>
            −
          </button>
          <button className="btn btn--icon" title={vi.orbit.recenter} aria-label={vi.orbit.recenter}
            onClick={() => setView(HOME_VIEW)}>
            ◎
          </button>
          {layout.customized && (
            <button className="btn btn--icon" title={vi.orbit.resetLayout} aria-label={vi.orbit.resetLayout}
              onClick={layout.reset}>
              ⟲
            </button>
          )}
          <button className={`btn btn--icon${still ? " is-on" : ""}`} aria-pressed={still}
            title={still ? vi.orbit.motionOff : vi.orbit.motionOn} aria-label={still ? vi.orbit.motionOff : vi.orbit.motionOn}
            onClick={() => setStill((v) => !v)}>
            {still ? "▶" : "❚❚"}
          </button>
        </div>
      </div>
      <SidePanel side="right" storageKey="opw.orbit.detail" min={240} max={640} initial={300}
        name={vi.side.detail} className="orbit-side">
        {(hide) => (
          <DetailPanel snapshot={snapshot} selection={selection} onSelect={setPicked} now={now}
            onStartTask={onStartTask} onOpenTask={onOpenTask} onOpenLink={onOpenLink}
            primary={primary} onGo={onGo} onHide={hide} />
        )}
      </SidePanel>
    </div>
  );
}
