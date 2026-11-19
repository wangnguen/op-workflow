// Pure helpers of the task window (no React), so they are easy to test.
import type { FindingView, GateKind, Severity, TaskDetail } from "../../ipc/types";

export type TaskTab = "progress" | "chat" | "brief" | "comments" | "plan" | "findings" | "diff" | "push";

const TABS: readonly TaskTab[] = ["progress", "chat", "brief", "comments", "plan", "findings", "diff", "push"];

/** A tab name from outside (an alert, the map), or `undefined` when it is not a tab. Alerts
 *  saved before the Tiến trình tab existed still say "log". */
export const asTab = (s: string | undefined): TaskTab | undefined =>
  s === "log" ? "progress" : TABS.find((tab) => tab === s);

/** The tab that shows what the task needs right now: a gate's own tab, the brief before the
 *  first step, otherwise the Tiến trình timeline. */
export function defaultTab(d: TaskDetail): TaskTab {
  if (d.gate?.kind === "planChanged") return "brief";
  if (d.gate?.kind === "planReview" || d.gate?.kind === "selfCode") return "plan";
  if (d.gate?.kind === "findings") return "findings";
  if (d.status === "awaitingPush" || d.status === "done") return "push";
  if (d.status === "running" || d.gate?.kind === "error" || d.gate?.kind === "afterFix") return "progress";
  if (
    d.findings.length > 0 &&
    (d.current === "fix" || d.current === "fixCross" || d.current === "crossReview" || d.current === "finalReview")
  ) {
    return "findings";
  }
  if (d.runs.length === 0 && !d.plan) return "brief";
  return "progress";
}

/** Where a gate's work is: the plan for the plan gate, the findings to pick, what the fix did… */
export function gateTab(kind: GateKind): TaskTab {
  switch (kind) {
    case "planReview":
      return "plan";
    case "findings":
      return "findings";
    case "error":
    case "afterFix":
      return "progress";
    default:
      return "brief";
  }
}

const SEVERITY_ORDER: Record<Severity, number> = { critical: 0, high: 1, medium: 2, low: 3 };

export const isSettled = (f: FindingView): boolean =>
  f.state === "fixed" || f.state === "accepted" || f.state === "deferred";

/** Open first, then by severity, file and line. */
export function sortFindings(list: FindingView[]): FindingView[] {
  return [...list].sort(
    (a, b) =>
      Number(isSettled(a)) - Number(isSettled(b)) ||
      SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity] ||
      a.file.localeCompare(b.file) ||
      (a.line ?? 0) - (b.line ?? 0),
  );
}

/** Picked for the next Sửa lỗi: the user pressed Sửa and it is still open (one already fixed and
 *  waiting for a review is not taken twice). Mirrors `to_fix` in flow.rs. */
export const isPicked = (f: FindingView): boolean => f.chosen === true && f.state === "open";

/** Findings nobody handled yet (`open`), and how many of them are picked for the AI. */
export function findingCounts(list: FindingView[]): { open: number; fix: number } {
  const open = list.filter((f) => f.state === "open");
  return { open: open.length, fix: open.filter(isPicked).length };
}

/** Open critical / high findings not picked yet (the "pick them all" shortcut). */
export const unpickedSevere = (list: FindingView[]): FindingView[] =>
  list.filter((f) => f.source !== "harness" && f.state === "open" && f.chosen !== true && (f.severity === "critical" || f.severity === "high"));

/** Open findings not picked yet ("Chọn thêm lỗi" after a fix). */
export const unpicked = (list: FindingView[]): number => list.filter((f) => f.state === "open" && f.chosen !== true).length;

/** `api/a.ts:42` */
export const findingPlace = (f: FindingView): string => (f.line ? `${f.file}:${f.line}` : f.file);
