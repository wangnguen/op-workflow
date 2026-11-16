import { describe, expect, it } from "vitest";
import type { RunSummary } from "../../ipc/types";
import { chatRun, lastEnded, openDenials, outputHead, runMs } from "./runs";

const run = (over: Partial<RunSummary>): RunSummary => ({
  id: 1,
  step: "analyze",
  agent: "claude",
  state: "done",
  startedAt: "2026-11-16T08:00:00Z",
  endedAt: "2026-11-16T08:00:50Z",
  denials: [],
  hasChat: true,
  ...over,
});

describe("Tiến trình and Chat helpers", () => {
  it("measures finished and running runs", () => {
    expect(runMs(run({}), 0)).toBe(50_000);
    const live = run({ state: "running", endedAt: undefined });
    expect(runMs(live, Date.parse("2026-11-16T08:02:00Z"))).toBe(120_000);
    expect(runMs(run({ state: "failed", endedAt: undefined }), 0)).toBeUndefined();
  });

  it("picks the run for the strip and the chat", () => {
    const runs = [run({ id: 1 }), run({ id: 2, state: "failed" }), run({ id: 3, state: "running", endedAt: undefined })];
    expect(lastEnded(runs)?.id).toBe(2);
    expect(lastEnded([])).toBeUndefined();
    expect(chatRun(runs)?.id).toBe(3);
    expect(chatRun(runs, 1)?.id).toBe(1);
    expect(chatRun(runs, 99)?.id).toBe(3);
    expect(chatRun(runs.slice(0, 2))?.id).toBe(2);
  });

  it("counts refused calls that still need the user", () => {
    const r = run({
      denials: [
        { tool: "Bash", input: "npx tsc", chained: false, rules: [{ rule: "npx tsc *", allowed: true }] },
        { tool: "Bash", input: "ls", chained: false, rules: [{ rule: "ls *", allowed: false }] },
        { tool: "Edit", input: "/etc/hosts", chained: false, rules: [] },
      ],
    });
    expect(openDenials(r)).toBe(2);
  });

  it("peeks at the first lines of a tool result", () => {
    expect(outputHead({ kind: "tool", text: "", output: "a\nb\nc" }, 2)).toEqual({ head: "a\nb", more: true });
    expect(outputHead({ kind: "tool", text: "", output: "a", outputCut: 10 })).toEqual({ head: "a", more: true });
    expect(outputHead({ kind: "tool", text: "" })).toEqual({ head: "", more: false });
  });
});
