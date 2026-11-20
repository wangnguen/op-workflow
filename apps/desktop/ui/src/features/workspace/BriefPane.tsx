// The Brief tab of a task's sessions: what the AI reads about the task (brief.md), or the
// task's OpenProject description while it has no brief. Read only here; "Sửa brief" opens the
// Brief tab of the task window, where it is edited.
import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, getTaskText, openTaskLink } from "../../ipc/client";
import type { TaskText } from "../../ipc/types";
import { Markdown } from "../../shared/Markdown";
import { Icon } from "../../shared/ui/Icon";
import "../task/task.css";

const t = vi.workspace.brief;
const tc = vi.flow.comments;

interface Props {
  wpId: number;
  onOpenTask: (wpId: number, tab: string) => void;
}

export function BriefPane({ wpId, onOpenTask }: Props) {
  const [text, setText] = useState<TaskText>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const alive = useRef(true);

  // the brief is on disk; OpenProject is asked only when there is no brief, or on "Đọc lại"
  const load = async (readNow: boolean) => {
    setLoading(true);
    setError(undefined);
    try {
      let v = await getTaskText(wpId, readNow);
      if (!v.brief && !readNow && alive.current) v = await getTaskText(wpId, true);
      if (alive.current) setText(v);
    } catch (e) {
      if (alive.current) setError(errorMessage(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  };

  useEffect(() => {
    alive.current = true;
    void load(false);
    return () => {
      alive.current = false;
    };
  }, [wpId]); // eslint-disable-line react-hooks/exhaustive-deps

  const open = (url: string) => {
    openTaskLink(wpId, url).catch((e: unknown) => setError(errorMessage(e)));
  };
  const brief = text?.brief?.trim();
  const description = text?.description.trim();

  return (
    <div className="task-pane ws-brief">
      <div className="comments-head">
        <span className="dim">{brief ? t.what : text ? t.noBrief : loading ? t.loading : ""}</span>
        <span className="comments-head-actions">
          {text?.taskUrl && (
            <button
              type="button"
              className="link-btn mono"
              title={`${tc.openTaskHint}\n${text.taskUrl}`}
              onClick={() => open(text.taskUrl as string)}
            >
              {tc.openProject} #{wpId}
              <Icon name="external" size={11} />
            </button>
          )}
          <button className="btn btn--ghost btn--sm" disabled={loading} onClick={() => void load(!brief)}>
            <Icon name="refresh" size={12} className={loading ? "is-spinning" : undefined} />
            {tc.refresh}
          </button>
          {brief && (
            <button className="btn btn--sm" title={t.editHint} onClick={() => onOpenTask(wpId, "brief")}>
              <Icon name="edit" size={12} />
              {t.edit}
            </button>
          )}
        </span>
      </div>

      {error && <p className="note note--error">{error}</p>}
      {!brief && text?.descriptionError && <p className="note note--warn">{text.descriptionError}</p>}
      {brief ? (
        <Markdown text={brief} className="ws-brief-text" />
      ) : description ? (
        <Markdown text={description} className="ws-brief-text" />
      ) : (
        text && !loading && !text.descriptionError && <p className="dim">{vi.board.peek.noDescription}</p>
      )}
    </div>
  );
}
