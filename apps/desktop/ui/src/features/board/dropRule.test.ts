import { describe, expect, it } from "vitest";
import type { StepRef, TaskColumn } from "../../ipc/types";
import { dropRule } from "./dropRule";

const step = (state: StepRef["state"], kind: StepRef["kind"] = "implement"): StepRef => ({ kind, index: 3, state, since: "" });
const kinds = (column: TaskColumn, s?: StepRef) =>
  (["inbox", "active", "awaitingPush", "done"] as const).map((to) => dropRule({ column, step: s }, to).kind);

describe("dropping a card on a column", () => {
  it("does nothing on its own column or outside the board", () => {
    expect(dropRule({ column: "active" }, "active")).toEqual({ kind: "none" });
    expect(dropRule({ column: "done" }, undefined)).toEqual({ kind: "none" });
  });

  it("starts an inbox task only in Đang làm", () => {
    expect(kinds("inbox")).toEqual(["none", "start", "refuse", "refuse"]);
    const r = dropRule({ column: "inbox" }, "done");
    expect(r.kind === "refuse" && r.reason).toContain("Bắt đầu task trước");
  });

  it("pauses or finishes a task in progress, never while a step runs", () => {
    expect(kinds("active", step("pending"))).toEqual(["pause", "none", "refuse", "finish"]);
    expect(kinds("active", step("running"))).toEqual(["refuse", "none", "refuse", "refuse"]);
    const pause = dropRule({ column: "active", step: step("running") }, "inbox");
    expect(pause.kind === "refuse" && pause.reason).toBe("Viết code đang chạy: bấm Dừng trước rồi mới tạm dừng.");
    const push = dropRule({ column: "active", step: step("failed") }, "awaitingPush");
    expect(push.kind === "refuse" && push.reason).toContain("tự sang Chờ push");
  });

  it("sends a task waiting for the push back or marks it done", () => {
    expect(kinds("awaitingPush", step("done", "pushSummary"))).toEqual(["pause", "continue", "none", "finish"]);
  });

  it("carries a finished task on, but not straight back to Inbox", () => {
    expect(kinds("done", step("done", "pushSummary"))).toEqual(["refuse", "continue", "continue", "none"]);
    const r = dropRule({ column: "done" }, "inbox");
    expect(r.kind === "refuse" && r.reason).toContain("kéo sang Đang làm");
  });
});
