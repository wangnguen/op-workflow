import { useRef, useState } from "react";
import { agentLabel, gateLabel, sessionStatusLabel, stepLabel, vi } from "../../i18n/vi";
import type { OrbitSnapshot } from "../../ipc/types";
import { useDismiss } from "../../shared/hooks";
import { STEP_COUNT } from "../../shared/steps";
import { DetailPanel } from "./DetailPanel";
import { Legend } from "./Legend";
import { HOME_VIEW, OrbitScene, type ViewState } from "./OrbitScene";
import { clampZoom } from "./geometry";
import { resolveSelection, type Selection } from "./selection";
import { useCompactionMarkers } from "./useCompactionMarkers";
import "./orbit.css";

interface Props {
  snapshot: OrbitSnapshot;
  now: number;
}

function statusLine(s: OrbitSnapshot): string {
  const a = s.active;
  if (!a) return vi.orbit.noActive;
  const live = a.steps.find((x) => x.state === "running" || x.state === "waitingUser");
  const parts = [a.taskId];
  if (live) parts.push(`${live.index}/${STEP_COUNT} ${stepLabel[live.kind]}${live.round && live.round > 1 ? ` ${vi.orbit.round(live.round)}` : ""}`);
  if (a.gate) parts.push(gateLabel[a.gate.kind]);
  const agent = a.agents[0];
  const child = agent?.children.find((c) => c.status === "running");
  const runningText = sessionStatusLabel.running.toLowerCase();
  if (child) parts.push(`${agentLabel[child.kind]} ${runningText}`);
  else if (agent?.status === "running") parts.push(`${agentLabel[agent.kind]} ${runningText}`);
  return parts.join(" · ");
}

export function OrbitView({ snapshot, now }: Props) {
  const [picked, setPicked] = useState<Selection | null>(null);
  const [view, setView] = useState<ViewState>(HOME_VIEW);
  const [still, setStill] = useState(false);
  const [legendOpen, setLegendOpen] = useState(false);
  const legendRef = useRef<HTMLDivElement>(null);
  useDismiss(legendRef, legendOpen, () => setLegendOpen(false));
  const markers = useCompactionMarkers(snapshot.active);
  const selection = resolveSelection(picked, snapshot);

  return (
    <div className={`orbit-view${still ? " is-still" : ""}`}>
      <div className="orbit-stage">
        <OrbitScene
          snapshot={snapshot}
          selection={selection}
          onSelect={setPicked}
          view={view}
          onView={setView}
          markers={markers}
          now={now}
        />

        <div className={`orbit-status${snapshot.active?.gate ? " is-waiting" : snapshot.active ? "" : " is-idle"}`}>
          <span className="orbit-status-dot" />
          {statusLine(snapshot)}
        </div>

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
          <button className={`btn btn--icon${still ? " is-on" : ""}`} aria-pressed={still}
            title={still ? vi.orbit.motionOff : vi.orbit.motionOn} aria-label={still ? vi.orbit.motionOff : vi.orbit.motionOn}
            onClick={() => setStill((v) => !v)}>
            {still ? "▶" : "❚❚"}
          </button>
        </div>
      </div>
      <DetailPanel snapshot={snapshot} selection={selection} onSelect={setPicked} now={now} />
    </div>
  );
}
