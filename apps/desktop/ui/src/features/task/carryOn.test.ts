import { describe, expect, it } from "vitest";
import type { FlowStatus, StepKind, StepState, StepView } from "../../ipc/types";
import { STEP_ORDER } from "../../shared/steps";
import { carryOnColumn, carryOnOptions } from "./carryOn";

/** Steps up to `upTo` done, the rest pending (`stateAt` overrides one). */
function detail(status: FlowStatus, upTo: StepKind, current: StepKind, stateAt?: [StepKind, StepState]) {
  const last = STEP_ORDER.indexOf(upTo);
  const steps: StepView[] = STEP_ORDER.map((kind, i) => ({
    kind,
    index: i + 1,
    state: stateAt?.[0] === kind ? stateAt[1] : i <= last ? "done" : "pending",
  }));
  // a clean first review never needed Sửa lỗi
  const fix = steps.find((s) => s.kind === "fix");
  if (fix && !stateAt) fix.state = "pending";
  return { status, steps, current };
}

describe("Làm tiếp", () => {
  it("offers to keep a finished task at the push summary when it goes back to Chờ push", () => {
    const d = detail("done", "pushSummary", "pushSummary");
    const o = carryOnOptions(d, "awaitingPush");
    expect(o.keep).toBe("pushSummary");
    expect(o.preferred).toBe("keep");
    expect(o.steps).toEqual(["analyze", "implement", "reviewLocal", "crossReview", "finalReview", "pushSummary"]);
    expect(carryOnColumn("keep", o.keep)).toBe("awaitingPush");
  });

  it("goes back to Viết code first when dropped on Đang làm", () => {
    const o = carryOnOptions(detail("done", "pushSummary", "pushSummary"), "active");
    expect(o.preferred).toBe("implement");
    expect(o.from).toBe("pushSummary");
    expect(carryOnColumn("implement", o.keep)).toBe("active");
  });

  it("only offers steps already reached, counting the one the task stands on", () => {
    // done at the plan gate: Phân tích waits for the user, nothing after it ran
    const o = carryOnOptions(detail("done", "intake", "analyze", ["analyze", "waitingUser"]), "active");
    expect(o).toMatchObject({ keep: "analyze", implement: false, steps: ["analyze"], preferred: "keep" });
    // ready at Viết code: shown as pending, but the task stands on it
    const ready = carryOnOptions(detail("done", "analyze", "implement"), "active");
    expect(ready.steps).toEqual(["analyze", "implement"]);
  });

  it("has no 'keep' for a task that is not done", () => {
    const o = carryOnOptions(detail("awaitingPush", "pushSummary", "pushSummary"), "active");
    expect(o.keep).toBeUndefined();
    expect(o.preferred).toBe("implement");
  });
});
