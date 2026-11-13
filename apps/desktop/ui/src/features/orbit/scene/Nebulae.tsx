import { vi } from "../../../i18n/vi";
import type { InboxItem } from "../../../ipc/types";
import { nebulaPositions } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  inbox: InboxItem[];
  selectedTaskId: string | undefined;
  onSelect: (taskId: string) => void;
}

const truncate = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

/** No active task: inbox tasks drift as dim nebulae waiting to ignite. */
export function Nebulae({ inbox, selectedTaskId, onSelect }: Props) {
  const spots = nebulaPositions(inbox.length);
  return (
    <g className="nebulae">
      <g className="idle-core" aria-hidden="true">
        <circle r={10} className="idle-ember" />
        <text y={-26} className="idle-title">
          {vi.orbit.noActive}
        </text>
        <text y={34} className="idle-hint">
          {vi.orbit.noActiveHint}
        </text>
      </g>
      {inbox.map((item, i) => {
        const p = spots[i];
        if (!p) return null;
        const selected = selectedTaskId === item.taskId;
        return (
          <g
            key={item.taskId}
            transform={`translate(${p.x} ${p.y})`}
            className={`nebula${selected ? " is-selected" : ""}`}
            {...interactive(`${item.taskId} ${item.title}`, () => onSelect(item.taskId), selected)}
          >
            <ellipse rx={62} ry={38} fill={`url(#g-nebula-${i % 3})`} filter="url(#f-soft)" className="nebula-cloud" />
            <ellipse rx={30} ry={18} fill={`url(#g-nebula-${i % 3})`} className="nebula-core" />
            <circle r={2} className="nebula-seed" />
            {selected && <ellipse rx={44} ry={28} className="select-ring" />}
            <text y={52} className="nebula-id">
              {item.taskId}
            </text>
            <text y={66} className="nebula-title">
              {truncate(item.title, 34)}
            </text>
          </g>
        );
      })}
    </g>
  );
}
