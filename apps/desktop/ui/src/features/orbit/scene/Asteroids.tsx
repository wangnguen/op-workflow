import { findingStateLabel, severityLabel } from "../../../i18n/vi";
import type { FindingView } from "../../../ipc/types";
import { asteroidLayout, asteroidShape, beltArcPath } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  findings: FindingView[];
  selectedFingerprint: string | undefined;
  onSelect: (fingerprint: string) => void;
}

/** Findings as an asteroid belt between the Review and Fix orbits. */
export function Asteroids({ findings, selectedFingerprint, onSelect }: Props) {
  if (findings.length === 0) return null;
  const layout = asteroidLayout(findings);
  return (
    <g className="belt">
      <path d={beltArcPath()} className="belt-ring" aria-hidden="true" />
      {findings.map((f) => {
        const p = layout.get(f.fingerprint);
        if (!p) return null;
        const selected = selectedFingerprint === f.fingerprint;
        const label = `${severityLabel[f.severity]}: ${f.title} (${findingStateLabel[f.state]})`;
        return (
          <g
            key={f.fingerprint}
            transform={`translate(${p.x} ${p.y})`}
            className={`rock${selected ? " is-selected" : ""}`}
            {...interactive(label, () => onSelect(f.fingerprint), selected)}
          >
            <circle r={p.size + 6} className="rock-hit" />
            <g transform={`rotate(${p.rotation})`}>
              <polygon
                points={asteroidShape(f.fingerprint, p.size)}
                className={`asteroid sev-${f.severity} asteroid--${f.state}`}
              />
            </g>
            {selected && <circle r={p.size + 5} className="select-ring" />}
            <title>{label}</title>
          </g>
        );
      })}
    </g>
  );
}
