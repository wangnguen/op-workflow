// "Làm tiếp": which ways back the task offers. Pure, mirrors `WorkflowEvent::Reopen` in Rust:
// only a step the task already reached can run again, never Nhận task.
import type { StepKind, TaskColumn, TaskDetail } from "../../ipc/types";

export type CarryOnChoice = "keep" | "implement" | "from";

export interface CarryOnOptions {
  /** the task is done: carry on at this step, where it stopped */
  keep?: StepKind;
  /** Viết code was reached: go back to it with a note */
  implement: boolean;
  /** steps it can run again from, in order */
  steps: StepKind[];
  preferred: CarryOnChoice;
  /** step picked first in "Chạy lại từ bước" */
  from?: StepKind;
}

export function carryOnOptions(d: Pick<TaskDetail, "status" | "steps" | "current">, to?: TaskColumn): CarryOnOptions {
  // a step waiting to run shows as pending, but the task stands on it
  const steps = d.steps
    .filter((s) => s.kind !== "intake" && (s.state !== "pending" || s.kind === d.current))
    .map((s) => s.kind);
  const keep = d.status === "done" ? (d.current ?? "pushSummary") : undefined;
  const implement = steps.includes("implement");
  const preferred: CarryOnChoice =
    keep && (to === "awaitingPush" || !implement) ? "keep" : implement ? "implement" : steps.length > 0 ? "from" : "keep";
  return { keep, implement, steps, preferred, from: steps[steps.length - 1] };
}

/** Column the task lands in after "Làm tiếp". */
export function carryOnColumn(choice: CarryOnChoice, keep: StepKind | undefined): TaskColumn {
  return choice === "keep" && keep === "pushSummary" ? "awaitingPush" : "active";
}
