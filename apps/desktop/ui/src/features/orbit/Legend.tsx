import { vi } from "../../i18n/vi";

type IconKey = (typeof vi.orbit.legendItems)[number][0];

function Icon({ kind }: { kind: IconKey }) {
  switch (kind) {
    case "star":
      return <circle r={7} className="lg-star" />;
    case "orbit":
      return <circle r={7} className="lg-orbit" />;
    case "planet":
      return (
        <>
          <circle cx={-5} r={3.5} className="lg-planet-done" />
          <circle cx={5} r={3.5} className="lg-planet-pending" />
        </>
      );
    case "ship":
      return <path d="M 8 0 L -5 5 L -2 0 L -5 -5 Z" className="lg-ship" />;
    case "atmosphere":
      return (
        <>
          <circle r={5} className="lg-atmo" />
          <circle r={8} className="lg-handoff" />
        </>
      );
    case "satellite":
      return (
        <>
          <circle cx={-4} r={2.5} className="lg-sat" />
          <circle cx={4} r={3.5} className="lg-moon" />
        </>
      );
    case "asteroid":
      return <polygon points="-6,-2 -2,-6 4,-5 7,0 3,6 -4,5" className="lg-rock" />;
    case "gate":
      return <path d="M 0 -7 L 7 0 L 0 7 L -7 0 Z" className="lg-gate" />;
    case "meteor":
      return (
        <>
          <line x1={-7} y1={-6} x2={4} y2={4} className="lg-meteor" />
          <circle cx={5} cy={5} r={2} className="lg-meteor-head" />
        </>
      );
    case "fuel":
      return <path d="M -7 3 A 8 8 0 0 1 7 3" className="lg-fuel" />;
    case "nebula":
      return <ellipse rx={8} ry={5} className="lg-nebula" />;
    case "distant":
      return <path d="M 0 -6 L 1.4 -1.4 L 6 0 L 1.4 1.4 L 0 6 L -1.4 1.4 L -6 0 L -1.4 -1.4 Z" className="lg-far" />;
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
            <svg viewBox="-10 -10 20 20" width={20} height={20} aria-hidden="true">
              <Icon kind={key} />
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
