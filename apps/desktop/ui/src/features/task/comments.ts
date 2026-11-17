import type { CommentView } from "../../ipc/types";

/** What the Comment tab head counts: other people's comments to look at, and the user's own. */
export function commentCounts(list: readonly CommentView[]): { fresh: number; mine: number } {
  let fresh = 0;
  let mine = 0;
  for (const c of list) {
    if (c.mine) mine += 1;
    else if (c.isNew) fresh += 1;
  }
  return { fresh, mine };
}

/** The folded line of the user's own comment: whitespace collapsed (CSS adds the "…"). */
export function oneLine(text: string): string {
  return text.split(/\s+/).filter(Boolean).join(" ");
}
