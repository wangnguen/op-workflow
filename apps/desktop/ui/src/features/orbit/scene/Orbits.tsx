import { vi } from "../../../i18n/vi";
import type { StepView } from "../../../ipc/types";
import { STEP_COUNT } from "../../../shared/steps";
import { orbitRadius, planetAngle, polar, shuttlePath, spiralPath, stepIndex } from "../geometry";

export interface CompactionMarker {
  stepIndex: number;
  count: number;
}

interface Props {
  steps: StepView[];
  /** 1-based index of the step that is running or waiting */
  current: number;
  markers: CompactionMarker[];
}

/** Orbit rings, the flight trail, the review⇄fix shuttle lane and compaction marks. */
export function Orbits({ steps, current, markers }: Props) {
  const review = steps.find((s) => s.kind === "crossReview");
  const fix = steps.find((s) => s.kind === "fix");
  const round = review?.round ?? 0;
  const inLoop = current === stepIndex("crossReview") || current === stepIndex("fix");
  const loopActive = review !== undefined && fix !== undefined && round > 0 && (inLoop || round > 1);
  const lane = shuttlePath(stepIndex("crossReview"), stepIndex("fix"));
  const reached = Math.max(1, current);

  return (
    <g className="orbits" aria-hidden="true">
      {steps.map((s) => (
        <circle
          key={s.kind}
          r={orbitRadius(s.index)}
          className={`orbit-ring orbit-ring--${s.state}${s.index === current ? " orbit-ring--current" : ""}`}
        />
      ))}

      <path d={spiralPath(1, STEP_COUNT)} className="trail trail--future" />
      {reached > 1 && <path d={spiralPath(1, reached)} className="trail trail--done" />}

      {loopActive && (
        <g className="shuttle">
          <path d={lane.d} className="shuttle-lane" />
          <g transform={`translate(${lane.mid.x} ${lane.mid.y})`}>
            <rect x={-26} y={-10} width={52} height={20} rx={10} className="shuttle-tag" />
            <text y={4} className="shuttle-text">
              {vi.orbit.round(round)}
            </text>
          </g>
        </g>
      )}

      {markers.map((m) => {
        const p = polar(orbitRadius(m.stepIndex), planetAngle(m.stepIndex) - 30);
        return (
          <g key={`${m.stepIndex}`} transform={`translate(${p.x} ${p.y})`} className="compaction">
            <path d="M 0 -6 L 6 0 L 0 6 L -6 0 Z" className="compaction-mark" />
            <text x={-10} y={4} className="compaction-text">
              {vi.orbit.compact(m.count)}
            </text>
          </g>
        );
      })}
    </g>
  );
}
