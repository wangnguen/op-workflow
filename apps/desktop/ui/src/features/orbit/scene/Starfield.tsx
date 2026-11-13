import { starfield } from "../geometry";

const DOTS = starfield(300);

/** Static background stars; a few twinkle. Lives outside the zoom group. */
export function Starfield() {
  return (
    <g aria-hidden="true">
      {DOTS.map((d, i) => (
        <circle
          key={i}
          cx={d.x}
          cy={d.y}
          r={d.r}
          className={d.twinkle ? "sf-dot sf-dot--twinkle" : "sf-dot"}
          style={d.twinkle ? { animationDelay: `${(i % 9) * 0.55}s` } : undefined}
        />
      ))}
    </g>
  );
}
