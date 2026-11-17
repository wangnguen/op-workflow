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
  if (d.gate?.kind === "planReview" || d.gate?.kind === "selfCode") return "plan";
  if (d.gate?.kind === "mediumTriage") return "findings";
  if (d.status === "awaitingPush" || d.status === "done") return "push";
  if (d.status === "running" || d.gate?.kind === "error") return "progress";
  if (
    d.findings.length > 0 &&
    (d.current === "fix" || d.current === "fixCross" || d.current === "crossReview" || d.current === "finalReview")
  ) {
    return "findings";
  }
  if (d.runs.length === 0 && !d.plan) return "brief";
  return "progress";
}

/** Where a gate's work is: the plan for the plan gate, the findings for triage, … */
export function gateTab(kind: GateKind): TaskTab {
  switch (kind) {
    case "planReview":
      return "plan";
    case "mediumTriage":
      return "findings";
    case "error":
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

/** Findings still open, and those the next fix step will take: picked ones, high and up, and
 *  mediums too unless they wait for the user's decision (`triage`). */
export function findingCounts(list: FindingView[], triage = false): { open: number; fix: number } {
  const open = list.filter((f) => !isSettled(f));
  const takes = (f: FindingView) =>
    f.chosen === true || f.severity === "critical" || f.severity === "high" || (f.severity === "medium" && !triage);
  return { open: open.length, fix: open.filter(takes).length };
}

/** Mediums of the final review nobody decided on yet (the triage gate waits for them). */
export const undecidedMediums = (list: FindingView[]): number =>
  list.filter((f) => !isSettled(f) && f.chosen !== true && f.severity === "medium").length;

/** `api/a.ts:42` */
export const findingPlace = (f: FindingView): string => (f.line ? `${f.file}:${f.line}` : f.file);
