import type { PushBlock, PushView } from "../../ipc/types";

/** The commit message and the PR description of a push summary. */
export interface PushParts {
  commit?: string;
  pr?: string;
}

const isCommit = (b: PushBlock) => /commit/i.test(b.label);
const isPr = (b: PushBlock) => /\bPR\b|pull request|プルリク|mô tả/i.test(b.label);

/** Find the two blocks by their heading; without headings, the first block is the commit
 *  message and the second the PR description. */
export function pushParts(push: PushView | undefined): PushParts {
  const blocks = push?.blocks ?? [];
  const commit = blocks.find(isCommit);
  const pr = blocks.find((b) => b !== commit && isPr(b));
  const rest = blocks.filter((b) => b !== commit && b !== pr);
  return {
    commit: (commit ?? rest.shift())?.text,
    pr: (pr ?? rest.shift())?.text,
  };
}
