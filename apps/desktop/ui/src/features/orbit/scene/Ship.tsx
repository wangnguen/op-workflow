import { agentLabel } from "../../../i18n/vi";
import type { AgentView } from "../../../ipc/types";
import { atmosphereRadius, handoffRingRadius, polar, shipHeading, shipLabelOffset, shipPosition } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  agent: AgentView;
  /** 1-based orbit the ship flies on */
  orbit: number;
  selectedId: string | undefined;
  onSelect: (agentId: string) => void;
}

// Ship art drawn by Codex (codex exec, 2026-11-13): nose toward +x, mirror-symmetric.
const SHIP = {
  wings: "M 5 -3.2 L -3.5 -9.5 L -9 -9.5 L -6.5 -3.2 Z M 5 3.2 L -3.5 9.5 L -9 9.5 L -6.5 3.2 Z",
  hull: "M 14 0 Q 9 -4.2 4 -4.2 L -4.5 -4.2 L -8 -2.8 L -8 2.8 L -4.5 4.2 L 4 4.2 Q 9 4.2 14 0 Z",
  engine: "M -11 -3 L -7 -3 L -6 -2 L -6 2 L -7 3 L -11 3 Z",
  cockpit: "M 10 0 Q 7 -2.4 2.5 -2 L 2.5 2 Q 7 2.4 10 0 Z",
  flame: "M -10.5 -2.2 Q -14 -3.5 -20 0 Q -14 3.5 -10.5 2.2 Z",
  flameCore: "M -10.5 -1.1 Q -13 -1.6 -16.5 0 Q -13 1.6 -10.5 1.1 Z",
} as const;
/** The art is drawn ~34 units long; a little bigger reads better at the default zoom. */
const SHIP_SCALE = 1.15;

/**
 * The agent as a ship. Its atmosphere is the context (dashed ring = handoff
 * threshold); subagents orbit as satellites, child agents (Codex) as violet moons.
 */
export function Ship({ agent, orbit, selectedId, onSelect }: Props) {
  const p = shipPosition(orbit);
  const atmo = atmosphereRadius(agent.contextTokens, agent.handoffAt);
  const ring = handoffRingRadius();
  const ratio = agent.handoffAt > 0 ? agent.contextTokens / agent.handoffAt : 0;
  const running = agent.subagents.filter((s) => s.status === "running");
  const selected = selectedId === agent.id;
  const pct = Math.round(ratio * 100);
  const label = shipLabelOffset(orbit, ring);

  return (
    <g className="ship-anchor" style={{ transform: `translate(${p.x}px, ${p.y}px)` }}>
      <g
        className={`ship agent-${agent.kind} ship--${agent.status}${selected ? " is-selected" : ""}`}
        {...interactive(`${agentLabel[agent.kind]} · context ${pct}%`, () => onSelect(agent.id), selected)}
      >
        <circle r={1} className="ship-atmo" fill={`url(#g-atmo-${agent.kind})`} style={{ transform: `scale(${atmo})` }} />
        <circle r={ring} className={`ship-handoff${ratio >= 0.9 ? " is-hot" : ""}`} />
        <circle r={ring + 4} className="ship-hit" />
        <g transform={`rotate(${shipHeading(orbit)}) scale(${SHIP_SCALE})`}>
          {agent.status === "running" && (
            <g className="ship-flame">
              <path d={SHIP.flame} className="ship-flame-outer" />
              <path d={SHIP.flameCore} className="ship-flame-core" />
            </g>
          )}
          <g filter="url(#f-glow)">
            <path d={SHIP.wings} className="ship-wings" />
            <path d={SHIP.engine} className="ship-engine" />
            <path d={SHIP.hull} className="ship-hull" />
            <path d={SHIP.cockpit} className="ship-cockpit" />
          </g>
        </g>
        {selected && <circle r={ring + 6} className="select-ring" />}
      </g>

      {running.length > 0 && (
        <g className="sat-orbit" aria-hidden="true">
          {running.map((s, k) => {
            const sp = polar(ring + 9, (k * 360) / running.length);
            return (
              <circle key={s.name} cx={sp.x} cy={sp.y} r={3.2} className="satellite">
                <title>{s.name}</title>
              </circle>
            );
          })}
        </g>
      )}

      {agent.children.map((child, k) => {
        const cp = polar(ring + 24, 205 + k * 50);
        const childSelected = selectedId === child.id;
        return (
          <g key={child.id} className="sat-orbit sat-orbit--slow">
            <g
              transform={`translate(${cp.x} ${cp.y})`}
              className={`moon agent-${child.kind} moon--${child.status}${childSelected ? " is-selected" : ""}`}
              {...interactive(`${agentLabel[child.kind]} (phiên con)`, () => onSelect(child.id), childSelected)}
            >
              {child.status === "running" && <circle r={11} className="moon-pulse" />}
              <circle r={6.5} className="moon-body" filter="url(#f-glow)" />
              {childSelected && <circle r={10} className="select-ring" />}
            </g>
          </g>
        );
      })}

      <text x={label.x} y={label.y} textAnchor={label.anchor} className="ship-label">
        {agentLabel[agent.kind]}
        {agent.children.some((c) => c.status === "running") ? ` + ${agentLabel.codex}` : ""} · {pct}%
      </text>
    </g>
  );
}
