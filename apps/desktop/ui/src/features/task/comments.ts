import { vi } from "../../i18n/vi";
import type { CommentsView, CommentView } from "../../ipc/types";
import { formatRelative } from "../../shared/format";

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

/** OpenProject leaves escaped separators in the raw comment. Keep a paragraph break there. */
export function openProjectCommentText(text: string): string {
  const lines: string[] = [];
  let fence: string | undefined;
  let separated = false;
  for (const line of text.split(/\r?\n/)) {
    const marker = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      if (marker?.[1]?.startsWith(fence) && !marker[2]?.trim()) fence = undefined;
    } else if (marker?.[1]) {
      fence = marker[1];
    } else if (/^ {0,3}\\?-{2,}[ \t]*$/.test(line)) {
      separated = true;
      continue;
    }
    if (separated) {
      separated = false;
      const previous = lines.at(-1);
      if (previous?.trim() && line.trim()) lines.push("");
      else if (previous !== undefined && !previous.trim() && !line.trim()) continue;
    }
    lines.push(line);
  }
  return lines.join("\n");
}

/** Why OpenProject could not be read, and how old the saved comments shown instead are. */
export function readProblem(view: CommentsView | undefined, now: number): string | undefined {
  if (!view?.error) return undefined;
  if (!view.savedAt) return view.error;
  return `${view.error} ${vi.flow.comments.savedCopy(formatRelative(view.savedAt, now))}`;
}

/** "Task chưa có comment nào": known to be empty, read now or from the saved copy. */
export function knownEmpty(view: CommentsView | undefined): boolean {
  return view !== undefined && view.comments.length === 0 && (!view.error || view.savedAt !== undefined);
}
