// One comment row, shared by the task window's Comment tab and the comment box of an Inbox card:
// other people's comments in full (new ones marked "cần xem"), the user's own folded to one line,
// each with a link that opens it in the browser.
import { useCallback, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import type { CommentView, CommentsView } from "../../ipc/types";
import { formatDateTime, formatRelative } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { oneLine } from "./comments";
import "./task.css";

const t = vi.flow.comments;

/** Where a comment's link goes. */
export type LinkWhere = "openProject" | "github";

/** Comments that were new when the view opened stay marked while it is open, even though the
 *  first read already counted them as seen (a second read, e.g. React's dev double effect or
 *  "Đọc lại", no longer reports them as new). */
export function useNewMarks(): (v: CommentsView) => CommentsView {
  const newIds = useRef(new Set<string>());
  return useCallback((v: CommentsView): CommentsView => {
    const mark = (c: CommentView): CommentView => {
      if (c.isNew) newIds.current.add(c.id);
      return { ...c, isNew: newIds.current.has(c.id) };
    };
    return {
      ...v,
      comments: v.comments.map(mark),
      issues: v.issues.map((i) => ({ ...i, comments: i.comments.map(mark) })),
    };
  }, []);
}

interface ItemProps {
  c: CommentView;
  now: number;
  who: string;
  where: LinkWhere;
  onOpen: (url: string) => void;
}

export function CommentItem(p: ItemProps) {
  const { c, now, who } = p;
  if (c.mine) return <OwnComment {...p} />;
  return (
    <li className={`comment${c.isNew ? " is-new" : ""}`}>
      <div className="comment-meta">
        <strong>{who}</strong>
        <span className="faint" title={formatDateTime(c.createdAt)}>
          {formatRelative(c.createdAt, now)}
        </span>
        {c.isNew && (
          <span className="chip chip--new" title={t.isNewHint}>
            {t.isNew}
          </span>
        )}
        <OpenLink {...p} />
      </div>
      <p className="comment-text">{c.text}</p>
    </li>
  );
}

/** "Mở trên OpenProject ↗" at the end of a comment's head; nothing when there is no link. */
function OpenLink({ c, where, onOpen }: ItemProps) {
  if (!c.url) return null;
  const url = c.url;
  return (
    <button type="button" className="link-btn comment-open" title={`${t.openOnHint(where)}\n${url}`} onClick={() => onOpen(url)}>
      {t.openOn(where)}
      <Icon name="external" size={11} />
    </button>
  );
}

/** The user's own comment: one folded line (who, when, start of the text); click to read it all.
 *  The row keeps its height while the text slides open below it. */
function OwnComment(p: ItemProps) {
  const { c, now, who } = p;
  const [open, setOpen] = useState(false);
  return (
    <li className={`comment comment--mine${open ? " is-open" : ""}`}>
      <button
        type="button"
        className="comment-fold"
        aria-expanded={open}
        title={open ? t.collapse : t.expand}
        onClick={() => setOpen((o) => !o)}
      >
        <span className={`diff-caret${open ? " is-open" : ""}`} aria-hidden="true">
          ▸
        </span>
        <strong>{who}</strong>
        <span className="faint" title={formatDateTime(c.createdAt)}>
          {formatRelative(c.createdAt, now)}
        </span>
        <span className="comment-excerpt faint" aria-hidden={open}>
          {oneLine(c.text)}
        </span>
      </button>
      <div className="comment-fold-body" inert={!open}>
        <div>
          <p className="comment-text">{c.text}</p>
          {c.url && (
            <div className="comment-own-foot">
              <OpenLink {...p} />
            </div>
          )}
        </div>
      </div>
    </li>
  );
}
