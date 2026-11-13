import { vi } from "../../../i18n/vi";
import type { TaskSystem } from "../../../ipc/types";
import { STAR_R } from "../geometry";
import { interactive } from "./interactive";

interface Props {
  task: TaskSystem;
  selected: boolean;
  onSelect: () => void;
}

/** The active task: a star whose glow turns amber while it waits for you. */
export function Star({ task, selected, onSelect }: Props) {
  const waiting = task.gate !== undefined;
  return (
    <g
      className={`star${waiting ? " star--waiting" : ""}${selected ? " is-selected" : ""}`}
      {...interactive(`${vi.orbit.panel.task} ${task.taskId}`, onSelect, selected)}
    >
      <circle r={STAR_R * 3.4} fill={waiting ? "url(#g-halo-wait)" : "url(#g-halo)"} className="star-halo" />
      <circle r={STAR_R} fill={waiting ? "url(#g-star-wait)" : "url(#g-star)"} className="star-core" filter="url(#f-glow)" />
      <circle r={STAR_R * 0.42} className="star-hot" />
      {selected && <circle r={STAR_R + 7} className="select-ring" />}
      <text y={STAR_R + 22} className="star-label">
        {task.taskId}
      </text>
      <text y={STAR_R + 36} className="star-sub">
        {task.repo}
      </text>
    </g>
  );
}
