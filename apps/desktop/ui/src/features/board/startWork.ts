// Pure helpers for the "Bắt đầu làm việc" dialog (no React, easy to test).
import type { BranchOption, RepoState } from "../../ipc/types";

/** Usual integration branches, most likely first. */
const COMMON_BASES = ["origin/development", "origin/develop", "origin/main", "origin/master", "development", "main", "master"];

/**
 * Base to pre-select:
 * 1. `origin/<branch>` when the task branch exists only on the remote (worked on elsewhere);
 * 2. the base used last time in this repo;
 * 3. a usual integration branch;
 * 4. the newest branch.
 */
export function pickDefaultBase(repo: RepoState, branch: string): string {
  const names = new Set(repo.branches.map((b) => b.name));
  const name = branch.trim();
  if (name && !names.has(name) && names.has(`origin/${name}`)) return `origin/${name}`;
  if (repo.lastBase && names.has(repo.lastBase)) return repo.lastBase;
  const common = COMMON_BASES.find((b) => names.has(b));
  return common ?? repo.branches[0]?.name ?? "";
}

export type BranchScope = "local" | "remote";

export const scopeOf = (b: BranchOption): BranchScope => (b.remote ? "remote" : "local");

/** Branches matching `filter` (case-insensitive substring), split into local and remote. */
export function splitBranches(branches: BranchOption[], filter: string): Record<BranchScope, BranchOption[]> {
  const q = filter.trim().toLowerCase();
  const out: Record<BranchScope, BranchOption[]> = { local: [], remote: [] };
  for (const b of branches) {
    if (!q || b.name.toLowerCase().includes(q)) out[scopeOf(b)].push(b);
  }
  return out;
}

/** "origin/feature/abc" → { prefix: "origin/feature/", leaf: "abc" }, to dim the folder part. */
export function branchParts(name: string): { prefix: string; leaf: string } {
  const cut = name.lastIndexOf("/") + 1;
  return { prefix: name.slice(0, cut), leaf: name.slice(cut) };
}

/** git's "ahead 2, behind 14" → "↑2 ↓14"; "gone" stays as is. */
export function trackText(track: string | undefined): string {
  if (!track) return "";
  const ahead = /ahead (\d+)/.exec(track)?.[1];
  const behind = /behind (\d+)/.exec(track)?.[1];
  if (!ahead && !behind) return track;
  return [ahead && `↑${ahead}`, behind && `↓${behind}`].filter(Boolean).join(" ");
}
