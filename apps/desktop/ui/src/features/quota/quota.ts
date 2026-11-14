import type { ProviderLink, QuotaWindow } from "../../ipc/types";

/** Remaining % of a window; a window whose reset time has passed counts as full. */
/** Window event that opens the quota popover (the tray icon and quota alerts send it). */
export const OPEN_QUOTA_EVENT = "opw:open-quota";

export function remainingPct(w: QuotaWindow | undefined, now: number): number {
  if (!w) return 0;
  if (w.resetsAt && Date.parse(w.resetsAt) <= now) return 100;
  return Math.min(100, Math.max(0, 100 - w.usedPct));
}

/** The link needs the user: an error, a missing CLI or a sign-in. */
export function linkProblem(link: ProviderLink | undefined): boolean {
  return link !== undefined && (link.status === "error" || link.status === "missing" || link.status === "signedOut");
}
