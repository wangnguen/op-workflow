import type { AlertView } from "../ipc/types";

/** Deleted alerts are excluded from the active lists and the unread badge. */
export type AlertFilter = "unread" | "all" | "deleted";

/** Active lists newest first (a restored alert goes back to its own time); Deleted by deletion time. */
export function visibleAlerts(items: readonly AlertView[], filter: AlertFilter): AlertView[] {
  if (filter === "deleted") return items.filter((a) => a.deletedAt)
    .sort((a, b) => Date.parse(b.deletedAt!) - Date.parse(a.deletedAt!));
  return items.filter((a) => !a.deletedAt && (filter === "all" || !a.read)).sort((a, b) => b.id - a.id);
}
