import type { AlertView } from "../ipc/types";

/** The bell's two lists: what the user has not opened yet, or the whole session. */
export type AlertFilter = "unread" | "all";

export function visibleAlerts(items: readonly AlertView[], filter: AlertFilter): AlertView[] {
  return filter === "unread" ? items.filter((a) => !a.read) : [...items];
}
