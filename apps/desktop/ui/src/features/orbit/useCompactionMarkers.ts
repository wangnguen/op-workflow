import { useRef } from "react";
import type { TaskSystem } from "../../ipc/types";
import { stepIndex } from "./geometry";
import type { CompactionMarker } from "./scene/Orbits";

/**
 * Remembers on which orbit each compaction happened, so the mark stays on the
 * flight path after the ship moves on. Snapshots only carry a running count.
 */
export function useCompactionMarkers(task: TaskSystem | undefined): CompactionMarker[] {
  const state = useRef<{ taskId: string | undefined; seen: number; markers: CompactionMarker[] }>({
    taskId: undefined,
    seen: 0,
    markers: [],
  });
  const s = state.current;
  const agent = task?.agents[0];
  if (!task || task.taskId !== s.taskId) {
    state.current = { taskId: task?.taskId, seen: agent?.compactions ?? 0, markers: [] };
    return state.current.markers;
  }
  const count = agent?.compactions ?? 0;
  if (agent && count > s.seen) {
    const idx = stepIndex(agent.step);
    const existing = s.markers.find((m) => m.stepIndex === idx);
    const markers = existing
      ? s.markers.map((m) => (m.stepIndex === idx ? { ...m, count: m.count + (count - s.seen) } : m))
      : [...s.markers, { stepIndex: idx, count: count - s.seen }];
    state.current = { taskId: s.taskId, seen: count, markers };
  } else if (count < s.seen) {
    state.current = { ...s, seen: count };
  }
  return state.current.markers;
}
