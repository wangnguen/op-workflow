import type { PriorityView } from "../../ipc/types";

/** Nebula colour when OpenProject gives the priority no colour (or the task has none). */
export const NEBULA_DEFAULT = "#8c7bff";

export interface NebulaTone {
  /** the priority's own colour on OpenProject, else the default violet */
  color: string;
  /** 0 (lowest priority) … 1 (highest); 0.5 when the order is unknown */
  weight: number;
  /** gradient opacity at the centre of the cloud and of its core */
  cloud: number;
  core: number;
}

const HEX = /^#[0-9a-f]{6}$/i;

/**
 * An Inbox task's nebula takes the colour of its priority on OpenProject and is stronger the
 * higher that priority sits in OpenProject's own order (Low faint … Immediate strong).
 */
export function nebulaTone(priority: PriorityView | undefined): NebulaTone {
  const color = priority?.color && HEX.test(priority.color) ? priority.color.toLowerCase() : NEBULA_DEFAULT;
  const { rank, ranks } = priority ?? {};
  const weight =
    rank !== undefined && ranks !== undefined && ranks > 1 && rank >= 1 && rank <= ranks ? (rank - 1) / (ranks - 1) : 0.5;
  const round2 = (n: number) => Math.round(n * 100) / 100;
  return { color, weight, cloud: round2(0.22 + 0.48 * weight), core: round2(0.35 + 0.5 * weight) };
}
