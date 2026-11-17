import type { Spin } from "../geometry";
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
  /** degrees the system has turned */
  rot?: Spin;
}

/** Orbit rings, the flight trail, the review⇄fix shuttle lane and compaction marks. */
export function Orbits({ steps, current, markers, rot = 0 }: Props) {
  // the review ⇄ fix loop: Review chéo (8) and the Sửa lỗi that fixes what it found (9)
  const review = steps.find((s) => s.kind === "crossReview");
  const fix = steps.find((s) => s.kind === "fixCross");
  const round = review?.round ?? 0;
  const inLoop = current === stepIndex("crossReview") || current === stepIndex("fixCross");
  const loopActive = review !== undefined && fix !== undefined && round > 0 && (inLoop || round > 1);
  const lane = shuttlePath(stepIndex("crossReview"), stepIndex("fixCross"), rot);
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

      <path d={spiralPath(1, STEP_COUNT, 14, rot)} className="trail trail--future" />
      {reached > 1 && <path d={spiralPath(1, reached, 14, rot)} className="trail trail--done" />}

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
        // a small diamond on the orbit behind the planet; a count (only when > 1) sits inside the
        // orbit, away from the planet's own label, which is pushed outward
        const angle = planetAngle(m.stepIndex, rot) - 30;
        const r = orbitRadius(m.stepIndex);
        const p = polar(r, angle);
        const inner = polar(r - 13, angle);
        return (
          <g key={`${m.stepIndex}`} className="compaction">
            <title>{vi.orbit.compact(m.count)}</title>
            <path d="M 0 -5 L 5 0 L 0 5 L -5 0 Z" transform={`translate(${p.x} ${p.y})`} className="compaction-mark" />
            {m.count > 1 && (
              <text x={inner.x} y={inner.y + 3} textAnchor="middle" className="compaction-text">
                {`×${m.count}`}
              </text>
            )}
          </g>
        );
      })}
    </g>
  );
}
