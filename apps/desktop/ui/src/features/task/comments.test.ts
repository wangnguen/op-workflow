import { describe, expect, it } from "vitest";
import { mockNewComments, mockTaskComments } from "../../ipc/mockTask";
import type { CommentsView, CommentView } from "../../ipc/types";
import { commentCounts, knownEmpty, oneLine, openProjectCommentText, readProblem } from "./comments";

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

  it("says when the comments shown are the saved copy", () => {
    const now = Date.parse("2026-11-17T10:00:00Z");
    const view = (over: Partial<CommentsView>): CommentsView => ({
      comments: [],
      issues: [],
      briefStale: false,
      fetchedAt: "2026-11-17T10:00:00Z",
      ...over,
    });
    expect(readProblem(undefined, now)).toBeUndefined();
    expect(readProblem(view({}), now)).toBeUndefined();
    expect(knownEmpty(view({}))).toBe(true);
    // offline, nothing saved: only the reason, and no "chưa có comment" (it is unknown)
    expect(readProblem(view({ error: "Mất mạng." }), now)).toBe("Mất mạng.");
    expect(knownEmpty(view({ error: "Mất mạng." }))).toBe(false);
    // offline with a saved read
    const saved = view({ error: "Mất mạng.", savedAt: "2026-11-17T09:55:00Z" });
    expect(readProblem(saved, now)).toBe("Mất mạng. Đang hiện bản đọc 5 phút trước.");
    expect(knownEmpty(saved)).toBe(true);
    expect(knownEmpty(view({ savedAt: "x", error: "e", comments: [c("1", false, false)] }))).toBe(false);
  });

  it("folds an own comment to one line", () => {
    expect(oneLine("  Đã sửa:\n\nkiểm tra   org null ")).toBe("Đã sửa: kiểm tra org null");
  });

  it("removes escaped OpenProject separators and keeps the bilingual paragraphs", () => {
    const raw = ["追加確認をお願いします。", "\\---  ", "API を確認", "", "/", "", "Nhờ kiểm tra.", "", "\\--", "", "Giữ `/api/user`."];
    expect(openProjectCommentText(raw.join("\r\n"))).toBe(
      "追加確認をお願いします。\n\nAPI を確認\n\n/\n\nNhờ kiểm tra.\n\nGiữ `/api/user`.",
    );
  });

  it("keeps hyphens in code, options, and list items", () => {
    const raw = ["Dùng --flag", "----", "- kiểm tra", "````text", "\\---", "```", "--", "````"];
    expect(openProjectCommentText(raw.join("\n"))).toBe(
      ["Dùng --flag", "", "- kiểm tra", "````text", "\\---", "```", "--", "````"].join("\n"),
    );
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
