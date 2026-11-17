import { describe, expect, it } from "vitest";
import { mockNewComments, mockTaskComments } from "../../ipc/mockTask";
import type { CommentView } from "../../ipc/types";
import { commentCounts, oneLine } from "./comments";

const c = (id: string, isNew: boolean, mine: boolean): CommentView => ({
  id,
  createdAt: "2026-11-17T01:00:00Z",
  text: id,
  isNew,
  mine,
});

describe("comment tab", () => {
  it("counts other people's new comments and the user's own apart", () => {
    expect(commentCounts([c("1", true, false), c("2", false, true), c("3", false, false)])).toEqual({ fresh: 1, mine: 1 });
    // an own comment is never "cần xem", even if flagged new by mistake
    expect(commentCounts([c("4", true, true)])).toEqual({ fresh: 0, mine: 1 });
    expect(commentCounts([])).toEqual({ fresh: 0, mine: 0 });
  });

  it("folds an own comment to one line", () => {
    expect(oneLine("  Đã sửa:\n\nkiểm tra   org null ")).toBe("Đã sửa: kiểm tra org null");
  });

  it("preview: own comments are shown but not counted as new", () => {
    const wpId = 990001;
    const badge = mockNewComments(wpId);
    const view = mockTaskComments(wpId, false);
    const all = [...view.comments, ...view.issues.flatMap((i) => i.comments)];
    expect(all.some((x) => x.mine)).toBe(true);
    expect(all.filter((x) => x.mine && x.isNew)).toEqual([]);
    expect(badge).toBe(all.filter((x) => x.isNew).length);
    mockTaskComments(wpId, true);
    expect(mockNewComments(wpId)).toBe(0);
  });
});
