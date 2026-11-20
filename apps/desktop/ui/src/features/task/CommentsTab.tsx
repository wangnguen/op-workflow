import { useCallback, useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, fetchIssues, getTaskComments, openTaskLink, refreshBriefComments } from "../../ipc/client";
import type { CommentView, CommentsView } from "../../ipc/types";
import { formatRelative } from "../../shared/format";
import { useNow } from "../../shared/hooks";
import { Icon } from "../../shared/ui/Icon";
import { CommentItem, useNewMarks } from "./CommentList";
import { commentCounts, knownEmpty, readProblem } from "./comments";

const t = vi.flow.comments;

interface Props {
  wpId: number;
  /** a step of the task runs now (the brief is not rewritten meanwhile) */
  running: boolean;
  /** counts on the card and the tab changed (comments seen, brief rewritten) */
  onChanged: () => void;
  /** Workspace: each comment gets "Hỏi Claude", which puts it into the chat box */
  onAsk?: (c: CommentView, who: string, text: string) => void;
}

/** OpenProject comments (read when the tab opens) and the GitHub issues of the last read. Other
 *  people's new comments are marked "cần xem" while the tab is open (opening it counts them as
 *  seen); the user's own comments are folded to one line and never count as new. */
export function CommentsTab({ wpId, running, onChanged, onAsk }: Props) {
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
  const remember = useNewMarks();

  const load = useCallback(() => {
    setLoading(true);
    setError(undefined);
    getTaskComments(wpId, true)
      .then((v) => {
        setView(remember(v));
        changedRef.current();
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setLoading(false));
  }, [wpId, remember]);

  useEffect(load, [load]);

  const updateBrief = () => {
    setBusy("brief");
    setError(undefined);
    refreshBriefComments(wpId)
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
    fetchIssues(wpId)
      .then((r) => getTaskComments(wpId, false).then((v) => [r, v] as const))
      .then(([r, v]) => {
        setView(remember(v));
        setNote([t.issueNewFound(r.newComments ?? 0), ...r.warnings].join(" · "));
        changed();
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(undefined));
  };

  const { fresh, mine } = commentCounts(view?.comments ?? []);
  const problem = readProblem(view, now);
  const open = (url: string) => {
    openTaskLink(wpId, url).catch((e: unknown) => setError(errorMessage(e)));
  };

  return (
    <div className="task-pane comments">
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
          <button className="btn btn--ghost btn--sm" disabled={loading || busy !== undefined} onClick={load}>
            <Icon name="refresh" size={12} />
            {t.refresh}
          </button>
        </span>
      </div>

      {error && <p className="note note--error">{error}</p>}
      {problem && <p className="note note--warn">{problem}</p>}
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

      {knownEmpty(view) && <p className="dim">{t.none}</p>}
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
                onAsk={onAsk}
              />
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
                <CommentItem key={c.id} c={c} now={now} who={c.mine ? t.you : "GitHub"} where="github" onOpen={open} onAsk={onAsk} />
              ))}
            </ul>
            {issue.unread > 0 && <p className="faint">{t.issueUnread(issue.unread)}</p>}
          </div>
        ))}
      </section>
    </div>
  );
}
