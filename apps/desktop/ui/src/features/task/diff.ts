// Pure helpers of the Diff tab (no React), so they are easy to test.
import type { DiffFile, DiffHunk, DiffLine, DiffView, FindingView } from "../../ipc/types";

/** `.\api\a.ts` → `api/a.ts`, so reviewer paths and git paths compare. */
export function normPath(p: string): string {
  return p.replace(/\\/g, "/").replace(/^\.\//, "").replace(/^\/+/, "");
}

/** Same file, even when one side is absolute or has an extra leading folder. */
export function samePath(finding: string, diff: string): boolean {
  const a = normPath(finding).toLowerCase();
  const b = normPath(diff).toLowerCase();
  return a === b || a.endsWith(`/${b}`) || b.endsWith(`/${a}`);
}

const settled = (f: FindingView) => f.state === "fixed" || f.state === "accepted" || f.state === "deferred";

/** Open findings of one file by working-tree line number. */
export function findingsByLine(findings: FindingView[], file: DiffFile): Map<number, FindingView[]> {
  const out = new Map<number, FindingView[]>();
  for (const f of findings) {
    if (settled(f) || f.line === undefined || !samePath(f.file, file.path)) continue;
    const list = out.get(f.line) ?? [];
    list.push(f);
    out.set(f.line, list);
  }
  return out;
}

/** Working-tree line numbers the diff shows for a file (added and context lines). */
function shownLines(file: DiffFile): Set<number> {
  const s = new Set<number>();
  for (const h of file.hunks) for (const l of h.lines) if (l.new !== undefined) s.add(l.new);
  return s;
}

/** Open findings no diff line can show (other files, or lines outside the hunks). */
export function outsideFindings(findings: FindingView[], diff: DiffView): FindingView[] {
  return findings.filter((f) => {
    if (settled(f)) return false;
    const file = diff.files.find((d) => samePath(f.file, d.path));
    if (!file) return true;
    return f.line === undefined || !shownLines(file).has(f.line);
  });
}

export interface SplitRow {
  left?: DiffLine;
  right?: DiffLine;
}

/** Side-by-side rows: context on both sides; a run of removed lines pairs with the added run
 *  that follows it. */
export function splitRows(hunk: DiffHunk): SplitRow[] {
  const rows: SplitRow[] = [];
  let removed: DiffLine[] = [];
  let added: DiffLine[] = [];
  const flush = () => {
    const n = Math.max(removed.length, added.length);
    for (let i = 0; i < n; i++) rows.push({ left: removed[i], right: added[i] });
    removed = [];
    added = [];
  };
  for (const line of hunk.lines) {
    if (line.kind === "removed") {
      if (added.length > 0) flush();
      removed.push(line);
    } else if (line.kind === "added") {
      added.push(line);
    } else {
      flush();
      rows.push({ left: line, right: line });
    }
  }
  flush();
  return rows;
}

/** Lines of a file; big files start collapsed. */
export const fileLines = (f: DiffFile): number => f.hunks.reduce((n, h) => n + h.lines.length, 0);
export const COLLAPSE_OVER = 400;
