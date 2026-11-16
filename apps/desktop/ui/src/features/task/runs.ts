// Pure helpers of the Tiến trình and Chat tabs (no React), so they are easy to test.
import type { ChatItem, RunSummary } from "../../ipc/types";

/** How long a run took, or has been running (`now` for a running one). */
export function runMs(run: RunSummary, now: number): number | undefined {
  const start = Date.parse(run.startedAt);
  const end = run.endedAt ? Date.parse(run.endedAt) : run.state === "running" ? now : Number.NaN;
  return Number.isNaN(start) || Number.isNaN(end) ? undefined : Math.max(0, end - start);
}

/** The last run that ended (for the status strip's "Phân tích xong: …"). */
export function lastEnded(runs: RunSummary[]): RunSummary | undefined {
  for (let i = runs.length - 1; i >= 0; i--) {
    const r = runs[i];
    if (r && r.state !== "running") return r;
  }
  return undefined;
}

/** Run to show in the Chat tab: the one picked, else the running one, else the latest. */
export function chatRun(runs: RunSummary[], picked?: number): RunSummary | undefined {
  return (
    (picked !== undefined ? runs.find((r) => r.id === picked) : undefined) ??
    runs.find((r) => r.state === "running") ??
    runs[runs.length - 1]
  );
}

/** Refused calls whose suggested rules are not all allowed yet (they need the user). */
export function openDenials(run: RunSummary): number {
  return run.denials.filter((d) => d.rules.length === 0 || d.rules.some((r) => !r.allowed)).length;
}

/** First `lines` lines of a tool result, and whether more is hidden behind "Kết quả". */
export function outputHead(item: ChatItem, lines = 4): { head: string; more: boolean } {
  const text = item.output ?? "";
  const all = text.split("\n");
  return { head: all.slice(0, lines).join("\n"), more: all.length > lines || (item.outputCut ?? 0) > 0 };
}
