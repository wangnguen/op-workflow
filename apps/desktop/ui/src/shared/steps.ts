import type { StepKind } from "../ipc/types";

/** Workflow order; mirrors `opw_domain::workflow::StepKind::ALL` (PLAN.md §4). */
export const STEP_ORDER: readonly StepKind[] = [
  "intake",
  "analyze",
  "implement",
  "checks",
  "harness",
  "reviewLocal",
  "fix",
  "crossReview",
  "fixCross",
  "browserTest",
  "finalReview",
  "pushSummary",
];

export const STEP_COUNT = STEP_ORDER.length;

/** 1-based position of a step, also its orbit number on the map. */
export const stepIndex = (kind: StepKind): number => STEP_ORDER.indexOf(kind) + 1;

/** Steps that only run when they are turned on (Kiểm tra, Harness, Test trình duyệt); until
 *  then the flow passes them as "bỏ qua". */
export const OPTIONAL_STEPS: readonly StepKind[] = ["checks", "harness", "browserTest"];

/** The two "Sửa lỗi" steps (7 after Tự review, 9 after Review chéo / Test trình duyệt / Review chốt). */
export const FIX_STEPS: readonly StepKind[] = ["fix", "fixCross"];

/** Steps an AI runs, as Settings → Các bước AI lists them; "fix" stands for both Sửa lỗi. */
export const AI_STEPS: readonly StepKind[] = [
  "analyze",
  "implement",
  "harness",
  "reviewLocal",
  "fix",
  "crossReview",
  "browserTest",
  "finalReview",
  "pushSummary",
];

/** "8" for Review chéo, "7 · 9" for Sửa lỗi (one setting covers both fix steps). */
export function stepNumber(kind: StepKind): string {
  return kind === "fix" || kind === "fixCross" ? FIX_STEPS.map(stepIndex).join(" · ") : String(stepIndex(kind));
}
