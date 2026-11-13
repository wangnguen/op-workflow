import type { QuotaWindow } from "../../ipc/types";

/** Remaining % of a window; a window whose reset time has passed counts as full. */
export function remainingPct(w: QuotaWindow | undefined, now: number): number {
  if (!w) return 0;
  if (w.resetsAt && Date.parse(w.resetsAt) <= now) return 100;
  return Math.min(100, Math.max(0, 100 - w.usedPct));
}
