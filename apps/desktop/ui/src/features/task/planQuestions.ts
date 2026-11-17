import type { PlanQuestion } from "../../ipc/types";

/** Answered: left to the AI, or a non-blank answer (as the engine counts it). */
export const isAnswered = (q: PlanQuestion): boolean => q.aiDecides === true || (q.answer ?? "").trim() !== "";

/** Questions the plan gate still waits for. */
export const unanswered = (questions: PlanQuestion[] | undefined): number =>
  (questions ?? []).filter((q) => !isAnswered(q)).length;
