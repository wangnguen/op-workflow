// "Xem task" on an Inbox card (and "N comment mới", and bell rows of Inbox tasks): the task's
// OpenProject description and its comments, without starting the task. Two tabs in a box of a
// fixed height, so switching moves nothing. Comments count as seen only once the Comment tab is
// shown, like the Comment tab of a started task.
import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, getTaskComments, getTaskText, openTaskLink } from "../../ipc/client";
import type { CommentsView, TaskText } from "../../ipc/types";
import { useNow } from "../../shared/hooks";
import { Markdown } from "../../shared/Markdown";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { CommentItem, useNewMarks } from "../task/CommentList";
import { commentCounts, knownEmpty, readProblem } from "../task/comments";
import "../task/flowDialogs.css";

const t = vi.flow.comments;
const pk = vi.board.peek;

export type PeekTab = "description" | "comments";

interface Props {
  wpId: number;
  taskId: string;
  title: string;
  /** the tab shown first: "N comment mới" opens the comments */
  tab?: PeekTab;
  onClose: () => void;
  /** comments were seen: the card's count changes */
  onChanged: () => void;
  /** "Bắt đầu làm việc" for this task (absent where the board is not at hand) */
  onStart?: () => void;
}

export function TaskPeek({ wpId, taskId, title, tab: first = "description", onClose, onChanged, onStart }: Props) {
  const [tab, setTab] = useState<PeekTab>(first);
  const [text, setText] = useState<TaskText>();
  const [view, setView] = useState<CommentsView>();
  const [loading, setLoading] = useState({ text: true, comments: true });
  const [error, setError] = useState<{ text?: string; comments?: string }>({});
  const now = useNow(30_000);
  const remember = useNewMarks();
  const changedRef = useRef(onChanged);
  changedRef.current = onChanged;
  const alive = useRef(true);
  // comments are marked seen once, when the Comment tab is first shown
  const seen = useRef(false);

  const loadText = () => {
    setLoading((l) => ({ ...l, text: true }));
    setError((e) => ({ ...e, text: undefined }));
    getTaskText(wpId, true)
      .then((v) => alive.current && setText(v))
      .catch((e: unknown) => alive.current && setError((x) => ({ ...x, text: errorMessage(e) })))
      .finally(() => alive.current && setLoading((l) => ({ ...l, text: false })));
  };
  const loadComments = (markSeen: boolean) => {
    setLoading((l) => ({ ...l, comments: true }));
    setError((e) => ({ ...e, comments: undefined }));
    if (markSeen) seen.current = true;
    getTaskComments(wpId, markSeen)
      .then((v) => {
        if (!alive.current) return;
        setView(remember(v));
        if (markSeen) changedRef.current();
      })
      .catch((e: unknown) => alive.current && setError((x) => ({ ...x, comments: errorMessage(e) })))
      .finally(() => alive.current && setLoading((l) => ({ ...l, comments: false })));
  };

  useEffect(() => {
    alive.current = true;
    loadText();
    loadComments(first === "comments");
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

  const pick = (next: PeekTab) => {
    setTab(next);
    if (next === "comments" && !seen.current) loadComments(true);
  };
  const reload = () => (tab === "comments" ? loadComments(true) : loadText());
  const open = (url: string) => {
    openTaskLink(wpId, url).catch((e: unknown) => setError((x) => ({ ...x, comments: errorMessage(e) })));
  };
  const { fresh, mine } = commentCounts(view?.comments ?? []);
  const problem = readProblem(view, now);
  const taskUrl = text?.taskUrl ?? view?.taskUrl;
  const busy = tab === "comments" ? loading.comments : loading.text;

  return (
    <div className="scrim flow-scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet flow-sheet peek-sheet" role="dialog" aria-modal="true" aria-labelledby="peek-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="info" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="peek-title">{taskId}</h2>
            <p>{title}</p>
          </div>
          <button className="btn btn--icon btn--ghost" aria-label={pk.close} title={pk.close} onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </header>

        <div className="peek-bar">
          <SegmentedControl
            label={pk.tabsLabel}
            value={tab}
            onChange={pick}
            options={[
              { value: "description", label: pk.tabs.description },
              {
                value: "comments",
                label: pk.tabs.comments,
                count: view ? view.comments.length : undefined,
                title: fresh > 0 ? t.newCount(fresh) : undefined,
              },
            ]}
          />
          {fresh > 0 && <span className="chip chip--new">{t.newCount(fresh)}</span>}
          <span className="comments-head-actions peek-bar-end">
            {taskUrl && (
              <button
                type="button"
                className="link-btn mono"
                title={`${t.openTaskHint}\n${taskUrl}`}
                onClick={() => open(taskUrl)}
              >
                {t.openProject} #{wpId}
                <Icon name="external" size={11} />
              </button>
            )}
            <button className="btn btn--ghost btn--sm" disabled={busy} onClick={reload}>
              <Icon name="refresh" size={12} className={busy ? "is-spinning" : undefined} />
              {t.refresh}
            </button>
          </span>
        </div>

        <div className="sheet-body peek-body" hidden={tab !== "description"}>
          {error.text && <p className="note note--error">{error.text}</p>}
          {text?.descriptionError && (
            <p className="note note--warn">
              {text.description ? pk.descriptionStale(text.descriptionError) : text.descriptionError}
            </p>
          )}
          {loading.text && !text && <p className="dim">{pk.loading}</p>}
          {text && !text.description.trim() && !text.descriptionError && <p className="dim">{pk.noDescription}</p>}
          {text?.description.trim() && <Markdown text={text.description} className="peek-text" />}
        </div>

        <div className="sheet-body peek-body" hidden={tab !== "comments"}>
          <p className="dim peek-count">
            {loading.comments && !view ? t.loading : view ? `${view.comments.length} comment` : ""}
            {mine > 0 && <span title={t.mineHint}> · {t.mineCount(mine)}</span>}
          </p>
          {error.comments && <p className="note note--error">{error.comments}</p>}
          {problem && <p className="note note--warn">{problem}</p>}
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
          <small className="faint">{text?.brief ? pk.parked : pk.notStarted}</small>
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
