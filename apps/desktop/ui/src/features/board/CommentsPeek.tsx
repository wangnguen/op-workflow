// "N comment mới" on a task that is not started yet (Inbox): a small box with the task's
// OpenProject comments, each with a link that opens it in the browser. Opening it counts them as
// seen, like the Comment tab of a started task.
import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, getTaskComments, openTaskLink } from "../../ipc/client";
import type { CommentsView } from "../../ipc/types";
import { useNow } from "../../shared/hooks";
import { Icon } from "../../shared/ui/Icon";
import { CommentItem, useNewMarks } from "../task/CommentList";
import { commentCounts } from "../task/comments";
import "../task/flowDialogs.css";

const t = vi.flow.comments;
const pk = vi.board.peek;

interface Props {
  wpId: number;
  taskId: string;
  title: string;
  onClose: () => void;
  /** comments were seen: the card's count changes */
  onChanged: () => void;
  /** "Bắt đầu làm việc" for this task (absent where the board is not at hand) */
  onStart?: () => void;
}

export function CommentsPeek({ wpId, taskId, title, onClose, onChanged, onStart }: Props) {
  const [view, setView] = useState<CommentsView>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const now = useNow(30_000);
  const remember = useNewMarks();
  const changedRef = useRef(onChanged);
  changedRef.current = onChanged;
  const alive = useRef(true);

  const load = () => {
    setLoading(true);
    setError(undefined);
    getTaskComments(wpId, true)
      .then((v) => {
        if (!alive.current) return;
        setView(remember(v));
        changedRef.current();
      })
      .catch((e: unknown) => alive.current && setError(errorMessage(e)))
      .finally(() => alive.current && setLoading(false));
  };

  useEffect(() => {
    alive.current = true;
    load();
    return () => {
      alive.current = false;
    };
  }, [wpId]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const open = (url: string) => {
    openTaskLink(wpId, url).catch((e: unknown) => setError(errorMessage(e)));
  };
  const { fresh, mine } = commentCounts(view?.comments ?? []);

  return (
    <div className="scrim flow-scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet flow-sheet peek-sheet" role="dialog" aria-modal="true" aria-labelledby="peek-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="info" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="peek-title">{pk.title(taskId)}</h2>
            <p title={title}>{title}</p>
          </div>
          <button className="btn btn--icon btn--ghost" aria-label={pk.close} title={pk.close} onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </header>

        <div className="sheet-body peek-body">
          <div className="comments-head">
            <span className="dim">
              {loading && !view ? t.loading : view ? `${view.comments.length} comment` : ""}
              {mine > 0 && <span title={t.mineHint}> · {t.mineCount(mine)}</span>}
              {fresh > 0 && <span className="chip chip--new">{t.newCount(fresh)}</span>}
            </span>
            <span className="comments-head-actions">
              {view?.taskUrl && (
                <button
                  type="button"
                  className="link-btn mono"
                  title={`${t.openTaskHint}\n${view.taskUrl}`}
                  onClick={() => open(view.taskUrl as string)}
                >
                  {t.openProject} #{wpId}
                  <Icon name="external" size={11} />
                </button>
              )}
              <button className="btn btn--ghost btn--sm" disabled={loading} onClick={load}>
                <Icon name="refresh" size={12} />
                {t.refresh}
              </button>
            </span>
          </div>

          {error && <p className="note note--error">{error}</p>}
          {view?.error && <p className="note note--warn">{view.error}</p>}
          {view && view.comments.length === 0 && !view.error && <p className="dim">{t.none}</p>}
          {view && view.comments.length > 0 && (
            <ul className="comment-list">
              {view.comments.map((c) => (
                <CommentItem
                  key={c.id}
                  c={c}
                  now={now}
                  who={c.mine ? t.you : (c.author ?? t.anonymous)}
                  where="openProject"
                  onOpen={open}
                />
              ))}
            </ul>
          )}
          {/* a task paused back to Inbox may have its GitHub issues read already */}
          {view?.issues.map((issue) => (
            <div key={issue.url} className="issue-thread">
              <div className="issue-title">
                <strong>{issue.title}</strong>
                <span className="faint mono" title={issue.url}>
                  #{issue.url.split("/").pop()}
                </span>
              </div>
              <ul className="comment-list">
                {issue.comments.map((c) => (
                  <CommentItem key={c.id} c={c} now={now} who={c.mine ? t.you : "GitHub"} where="github" onOpen={open} />
                ))}
              </ul>
            </div>
          ))}
        </div>

        <footer className="sheet-foot">
          <small className="faint">{pk.notStarted}</small>
          <span className="flow-actions">
            {onStart && (
              <button
                className="btn btn--sm"
                onClick={() => {
                  onClose();
                  onStart();
                }}
              >
                {vi.board.start}
              </button>
            )}
            <button className="btn btn--primary btn--sm" onClick={onClose}>
              {pk.close}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
