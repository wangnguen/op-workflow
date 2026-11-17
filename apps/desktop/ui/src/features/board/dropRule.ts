// What dropping a task card on another column of the Task board does. Pure, so every pair of
// columns is tested; the board only carries the answer out (open a dialog, pause, refuse).
import { stepLabel, vi } from "../../i18n/vi";
import type { TaskCard, TaskColumn } from "../../ipc/types";

export type DropAction =
  /** same column, or not over a column: the card just goes back */
  | { kind: "none" }
  /** Inbox → Đang làm: the "Bắt đầu làm việc" dialog */
  | { kind: "start" }
  /** back to Inbox: "Tạm dừng" (also out of Đã xong; starting it again runs a new round) */
  | { kind: "pause" }
  /** into Đã xong: the push check, then "Đã push xong" */
  | { kind: "finish" }
  /** out of Đã xong, or Chờ push → Đang làm: "Làm tiếp" asks from where */
  | { kind: "continue" }
  /** not allowed: the card slides back and the reason shows */
  | { kind: "refuse"; reason: string };

const d = vi.board.drop;

export function dropRule(card: Pick<TaskCard, "column" | "step">, to: TaskColumn | undefined): DropAction {
  const from = card.column;
  if (!to || to === from) return { kind: "none" };
  const running = card.step?.state === "running" ? stepLabel[card.step.kind] : undefined;
  switch (from) {
    case "inbox":
      return to === "active" ? { kind: "start" } : { kind: "refuse", reason: d.startFirst };
    case "active":
      if (to === "awaitingPush") return { kind: "refuse", reason: d.pushByItself };
      if (running) return { kind: "refuse", reason: to === "inbox" ? d.stopToPause(running) : d.stopToFinish(running) };
      return to === "inbox" ? { kind: "pause" } : { kind: "finish" };
    case "awaitingPush":
      if (to === "active") return { kind: "continue" };
      if (running) return { kind: "refuse", reason: to === "inbox" ? d.stopToPause(running) : d.stopToFinish(running) };
      return to === "inbox" ? { kind: "pause" } : { kind: "finish" };
    case "done":
      return to === "inbox" ? { kind: "pause" } : { kind: "continue" };
  }
}
