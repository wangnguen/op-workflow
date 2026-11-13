import { agentLabel, sessionStatusLabel, vi } from "../../../i18n/vi";
import type { DistantSession } from "../../../ipc/types";
import type { ItemDragStart } from "../OrbitScene";
import { distantPositions } from "../geometry";
import type { LayoutOverrides } from "../useLayout";
import { interactive } from "./interactive";

interface Props {
  sessions: DistantSession[];
  selectedId: string | undefined;
  onSelect: (id: string) => void;
  layout: LayoutOverrides;
  onDragStart: ItemDragStart;
}

const SPARKLE = "M 0 -8 L 1.8 -1.8 L 8 0 L 1.8 1.8 L 0 8 L -1.8 1.8 L -8 0 L -1.8 -1.8 Z";

/** Sessions opened outside the app, as small far-away stars at the edge. Drag to rearrange. */
export function DistantStars({ sessions, selectedId, onSelect, layout, onDragStart }: Props) {
  const spots = distantPositions(sessions.length);
  return (
    <g className="distant">
      {sessions.map((s, i) => {
        const spot = spots[i];
        if (!spot) return null;
        const key = `distant:${s.id}`;
        const p = layout[key] ?? spot;
        const selected = selectedId === s.id;
        return (
          <g
            key={s.id}
            transform={`translate(${p.x} ${p.y})`}
            className={`far agent-${s.agent} far--${s.status}${selected ? " is-selected" : ""}`}
            onPointerDown={onDragStart(key, p)}
            {...interactive(`${s.label} · ${vi.sessions.outside}`, () => onSelect(s.id), selected)}
          >
            <circle r={14} className="far-hit" />
            <path d={SPARKLE} className="far-body" />
            {selected && <circle r={12} className="select-ring" />}
            <text y={24} className="far-label">
              {s.label}
            </text>
            <text y={37} className="far-sub">
              {agentLabel[s.agent]} · {sessionStatusLabel[s.status]}
            </text>
          </g>
        );
      })}
    </g>
  );
}
