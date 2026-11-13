import { stepLabel, stepStateLabel, vi } from "../../../i18n/vi";
import type { StepKind, StepView } from "../../../ipc/types";
import { PLANET_R, labelPosition, planetPosition } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  steps: StepView[];
  selectedStep: StepKind | undefined;
  onSelect: (step: StepKind) => void;
}

/** One planet per workflow step: lit when done, pulsing when live, dim when pending. */
export function Planets({ steps, selectedStep, onSelect }: Props) {
  return (
    <g className="planets">
      {steps.map((s) => {
        const p = planetPosition(s.index);
        const l = labelPosition(s.index);
        const live = s.state === "running" || s.state === "waitingUser";
        const selected = selectedStep === s.kind;
        const sub =
          s.round && s.round > 1 && (s.kind === "crossReview" || s.kind === "fix")
            ? `${stepStateLabel[s.state]} · ${vi.orbit.round(s.round)}`
            : stepStateLabel[s.state];
        return (
          <g
            key={s.kind}
            transform={`translate(${p.x} ${p.y})`}
            className={`planet planet--${s.state}${selected ? " is-selected" : ""}`}
            {...interactive(`${s.index}. ${stepLabel[s.kind]}: ${sub}`, () => onSelect(s.kind), selected)}
          >
            {live && <circle r={PLANET_R * 2.3} className="planet-pulse" />}
            <circle r={PLANET_R + 7} className="planet-hit" />
            <circle r={PLANET_R} className="planet-body" />
            {s.state === "done" && <path d="M -3.6 0.2 L -1.1 2.7 L 3.8 -2.6" className="planet-check" />}
            {selected && <circle r={PLANET_R + 5} className="select-ring" />}
            <text x={l.x - p.x} y={l.y - p.y + l.mainDy} textAnchor={l.anchor} className="planet-label">
              {s.index}. {stepLabel[s.kind]}
            </text>
            <text x={l.x - p.x} y={l.y - p.y + l.subDy} textAnchor={l.anchor} className="planet-sub">
              {sub}
            </text>
          </g>
        );
      })}
    </g>
  );
}
