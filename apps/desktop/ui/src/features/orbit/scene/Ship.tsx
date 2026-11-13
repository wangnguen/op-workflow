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

const HULL = "M 12 0 L -7 7.5 L -3 0 L -7 -7.5 Z";
const FLAME = "M -4.5 -3.2 L -14 0 L -4.5 3.2 Z";

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
        <g transform={`rotate(${shipHeading(orbit)})`}>
          {agent.status === "running" && <path d={FLAME} className="ship-flame" />}
          <path d={HULL} className="ship-hull" filter="url(#f-glow)" />
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
