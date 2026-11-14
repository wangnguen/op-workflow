import { useCallback, useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, fetchIssues, getTaskComments, refreshBriefComments } from "../../ipc/client";
import type { CommentView, CommentsView, TaskDetail } from "../../ipc/types";
import { formatDateTime, formatRelative } from "../../shared/format";
import { useNow } from "../../shared/hooks";
import { Icon } from "../../shared/ui/Icon";

const t = vi.flow.comments;

interface Props {
  detail: TaskDetail;
  /** counts on the card and the tab changed (comments seen, brief rewritten) */
  onChanged: () => void;
}

/** OpenProject comments (read when the tab opens) and the GitHub issues of the last read. The
 *  "mới" marks stay while the tab is open; opening it counts them as seen. */
export function CommentsTab({ detail: d, onChanged }: Props) {
  const [view, setView] = useState<CommentsView>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<"brief" | "issue">();
  const [error, setError] = useState<string>();
  const [note, setNote] = useState<string>();
  const now = useNow(30_000);
  // the parent's callback may change every render; loading must not follow it
  const changedRef = useRef(onChanged);
  changedRef.current = onChanged;
  const changed = () => changedRef.current();
  // comments that were new when this tab opened stay marked while it is open, even though the
  // first read already counted them as seen (and a second read, e.g. React's dev double effect
  // or "Đọc lại", no longer reports them as new)
  const newIds = useRef(new Set<string>());
  const remember = useCallback((v: CommentsView): CommentsView => {
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

  const load = useCallback(() => {
    setLoading(true);
    setError(undefined);
    getTaskComments(d.wpId, true)
      .then((v) => {
        setView(remember(v));
        changedRef.current();
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setLoading(false));
  }, [d.wpId, remember]);

  useEffect(load, [load]);

  const updateBrief = () => {
    setBusy("brief");
    setError(undefined);
    refreshBriefComments(d.wpId)
      .then((v) => {
        setView(remember(v));
        setNote(t.briefUpdated);
        changed();
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(undefined));
  };

  const refetchIssue = () => {
    setBusy("issue");
    setError(undefined);
    setNote(undefined);
    fetchIssues(d.wpId)
      .then((r) => getTaskComments(d.wpId, false).then((v) => [r, v] as const))
      .then(([r, v]) => {
        setView(remember(v));
        setNote([t.issueNewFound(r.newComments ?? 0), ...r.warnings].join(" · "));
        changed();
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(undefined));
  };

  const fresh = view ? view.comments.filter((c) => c.isNew).length : 0;
  const running = d.status === "running";

  return (
    <div className="task-pane comments">
      <div className="comments-head">
        <span className="dim">
          {loading && !view ? t.loading : view ? `${view.comments.length} comment` : ""}
          {fresh > 0 && <span className="chip chip--new">{t.newCount(fresh)}</span>}
        </span>
        <span className="comments-head-actions">
          {view?.taskUrl && (
            <span className="faint mono" title={view.taskUrl}>
              {t.openProject} #{d.wpId}
            </span>
          )}
          <button className="btn btn--ghost btn--sm" disabled={loading || busy !== undefined} onClick={load}>
            <Icon name="refresh" size={12} />
            {t.refresh}
          </button>
        </span>
      </div>

      {error && <p className="note note--error">{error}</p>}
      {view?.error && <p className="note note--warn">{view.error}</p>}
      {note && <p className="note">{note}</p>}

      {view?.briefStale && (
        <div className="note note--warn comments-stale">
          <Icon name="alert" size={13} />
          <span>{t.briefStale}</span>
          <button
            className="btn btn--primary btn--sm"
            disabled={busy !== undefined || running}
            title={t.updateBriefHint}
            onClick={updateBrief}
          >
            {t.updateBrief}
          </button>
        </div>
      )}

      {view && view.comments.length === 0 && !view.error && <p className="dim">{t.none}</p>}
      {view && view.comments.length > 0 && (
        <ul className="comment-list">
          {view.comments.map((c) => (
            <CommentItem key={c.id} c={c} now={now} who={c.author ?? t.anonymous} />
          ))}
        </ul>
      )}

      <section className="comments-issues">
        <header>
          <h4>
            {t.issues}
            {view?.issuesReadAt && <span className="faint"> · {t.issuesRead(formatRelative(view.issuesReadAt, now))}</span>}
          </h4>
          <button
            className="btn btn--sm"
            disabled={busy !== undefined}
            title={t.issueRefreshHint}
            onClick={refetchIssue}
          >
            <Icon name="refresh" size={12} />
            {busy === "issue" ? "…" : t.issueRefresh}
          </button>
        </header>
        {view && view.issues.length === 0 && <p className="dim">{t.issuesNone}</p>}
        {view?.issues.map((issue) => (
          <div key={issue.url} className="issue-thread">
            <div className="issue-title">
              <strong>{issue.title}</strong>
              <span className="faint mono" title={issue.url}>
                #{issue.url.split("/").pop()}
              </span>
            </div>
            {issue.comments.length === 0 && <p className="dim">{t.issueNoComments}</p>}
            <ul className="comment-list">
              {issue.comments.map((c) => (
                <CommentItem key={c.id} c={c} now={now} who="GitHub" />
              ))}
            </ul>
            {issue.unread > 0 && <p className="faint">{t.issueUnread(issue.unread)}</p>}
          </div>
        ))}
      </section>
    </div>
  );
}

function CommentItem({ c, now, who }: { c: CommentView; now: number; who: string }) {
  return (
    <li className={`comment${c.isNew ? " is-new" : ""}`}>
      <div className="comment-meta">
        <strong>{who}</strong>
        <span className="faint" title={formatDateTime(c.createdAt)}>
          {formatRelative(c.createdAt, now)}
        </span>
        {c.isNew && <span className="chip chip--new">{t.isNew}</span>}
      </div>
      <p className="comment-text">{c.text}</p>
    </li>
  );
}
