import { agentLabel, vi } from "../../../i18n/vi";
import type { QuotaSnapshot } from "../../../ipc/types";
import { fuelArcs } from "../geometry";

interface Props {
  quota: QuotaSnapshot[];
  now: number;
}

/** Outer ring: four fuel arcs for the Claude and Codex quota windows. */
export function FuelRing({ quota, now }: Props) {
  return (
    <g className="fuel" aria-label={vi.quota.title}>
      {fuelArcs(quota, now).map((a) => {
        const low = a.known && a.remainingPct < 20;
        const kind = a.kind === "fiveHour" ? vi.quota.fiveHour : vi.quota.sevenDay;
        return (
          <g key={a.key} className={`fuel-arc fuel-${a.provider}${low ? " is-low" : ""}`}>
            <path d={a.track} className="fuel-track" />
            {a.fill && <path d={a.fill} className="fuel-fill" />}
            <text x={a.label.x} y={a.label.y} textAnchor={a.labelAnchor} className="fuel-label">
              <tspan className="fuel-name">
                {agentLabel[a.provider]} {kind}
              </tspan>
              <tspan className="fuel-value" dx={6}>
                {a.known ? `${Math.round(a.remainingPct)}%` : "?"}
              </tspan>
            </text>
          </g>
        );
      })}
    </g>
  );
}
