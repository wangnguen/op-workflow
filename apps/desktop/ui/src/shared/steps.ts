import type { StepKind } from "../ipc/types";

/** Workflow order; mirrors `opw_domain::workflow::StepKind::ALL` (PLAN.md §4). */
export const STEP_ORDER: readonly StepKind[] = [
  "intake",
  "analyze",
  "implement",
  "reviewLocal",
  "fix",
  "crossReview",
  "finalReview",
  "pushSummary",
];

export const STEP_COUNT = STEP_ORDER.length;
