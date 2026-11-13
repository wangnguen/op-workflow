import type { OrbitSnapshot, TaskSystem } from "../../ipc/types";

/** Every task in "Đang làm", most recent first. */
export function systemsOf(s: OrbitSnapshot): TaskSystem[] {
  return [s.active, ...(s.others ?? [])].filter((x): x is TaskSystem => x !== undefined);
}

/** The snapshot with `taskId`'s system in the centre; unknown ids keep the most recent one. */
export function focusSystem(s: OrbitSnapshot, taskId: string | null): OrbitSnapshot {
  const all = systemsOf(s);
  const focus = all.find((x) => x.taskId === taskId);
  if (!focus || focus === s.active) return s;
  return { ...s, active: focus, others: all.filter((x) => x !== focus) };
}
