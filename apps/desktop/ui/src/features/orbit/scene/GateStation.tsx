import type { Spin } from "../geometry";
import { gateLabel } from "../../../i18n/vi";
import type { GateView } from "../../../ipc/types";
import { gateLabelOffset, gatePosition } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  gate: GateView;
  stepIndex: number;
  selected: boolean;
  onSelect: () => void;
  /** degrees the system has turned */
  rot?: Spin;
}

/** Glowing amber station on the orbit where the workflow waits for you. */
export function GateStation({ gate, stepIndex, selected, onSelect, rot = 0 }: Props) {
  const p = gatePosition(stepIndex, rot);
  const label = gateLabelOffset(stepIndex, rot);
  return (
    <g
      transform={`translate(${p.x} ${p.y})`}
      className={`gate${selected ? " is-selected" : ""}`}
      {...interactive(gateLabel[gate.kind], onSelect, selected)}
    >
      <circle r={20} className="gate-glow" />
      <circle r={13} className="gate-ring" />
      <path d="M 0 -9 L 9 0 L 0 9 L -9 0 Z" className="gate-body" filter="url(#f-glow)" />
      <text y={3.5} className="gate-mark">
        !
      </text>
      <text x={label.x} y={label.y} textAnchor={label.anchor} className="gate-label">
        {gateLabel[gate.kind]}
      </text>
    </g>
  );
}
