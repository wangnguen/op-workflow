import { vi } from "../../i18n/vi";

type IconKey = (typeof vi.orbit.legendItems)[number][0];

/** One plain shape per item, same size and weight, so the list reads at a glance. */
function Glyph({ kind }: { kind: IconKey }) {
  switch (kind) {
    case "star":
      return <circle r={5} className="lg-star" />;
    case "orbit":
      return <circle r={6.5} className="lg-line" />;
    case "planet":
      return <circle r={4} className="lg-planet" />;
    case "ship":
      return <path d="M 6 0 L -4.5 4.5 L -4.5 -4.5 Z" className="lg-ship" />;
    case "atmosphere":
      return <circle r={6.5} className="lg-atmo" />;
    case "satellite":
      return <circle r={2.6} className="lg-sat" />;
    case "asteroid":
      return <path d="M 0 -5 L 4.8 -1.5 L 3 4 L -3 4 L -4.8 -1.5 Z" className="lg-rock" />;
    case "gate":
      return <path d="M 0 -5.5 L 5.5 0 L 0 5.5 L -5.5 0 Z" className="lg-gate" />;
    case "compaction":
      return <path d="M 0 -4 L 4 0 L 0 4 L -4 0 Z" className="lg-compact" />;
    case "meteor":
      return <path d="M -5 -5 L 4.5 4.5" className="lg-meteor" />;
    case "fuel":
      return <path d="M -6 3 A 6.5 6.5 0 0 1 6 3" className="lg-fuel" />;
    case "nebula":
      return <ellipse rx={6.5} ry={4} className="lg-nebula" />;
    case "distant":
      return <path d="M 0 -5 V 5 M -5 0 H 5" className="lg-far" />;
  }
}

/** "Cách đọc bản đồ": what each body on the map means. */
export function Legend() {
  return (
    <div className="legend" role="dialog" aria-label={vi.orbit.legend}>
      <h3>{vi.orbit.legend}</h3>
      <ul>
        {vi.orbit.legendItems.map(([key, name, meaning]) => (
          <li key={key}>
            <svg className="legend-ico" viewBox="-10 -10 20 20" width={22} height={22} aria-hidden="true">
              <Glyph kind={key} />
            </svg>
            <span>
              <b>{name}</b> <span className="dim">{meaning}</span>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
