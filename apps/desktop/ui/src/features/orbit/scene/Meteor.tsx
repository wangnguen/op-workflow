import { vi } from "../../../i18n/vi";
import { STAR_R } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  count: number;
  onSelect: () => void;
}

/** New OpenProject comments: a meteor streaks toward the star and leaves a badge. */
export function Meteor({ count, onSelect }: Props) {
  if (count <= 0) return null;
  return (
    <g>
      <g className="meteor" aria-hidden="true">
        <line x1={0} y1={0} x2={70} y2={-66} stroke="url(#g-meteor)" className="meteor-tail" />
        <circle r={2.4} className="meteor-head" />
      </g>
      <g
        // geometry's COMMENT_BADGE_BOX keeps ship and gate labels off this badge
        transform={`translate(${STAR_R + 10} ${-STAR_R - 14})`}
        className="comment-badge"
        {...interactive(vi.orbit.newComment(count), onSelect)}
      >
        <rect x={0} y={-10} width={84} height={20} rx={10} />
        <text x={42} y={4}>
          {vi.orbit.newComment(count)}
        </text>
      </g>
    </g>
  );
}
