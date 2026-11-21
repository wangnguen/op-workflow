import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, getTaskOutput, stopSessionTask } from "../../ipc/client";
import type { BackgroundTask, SessionView, TaskOutput } from "../../ipc/types";
import { formatDuration, formatTokens } from "../../shared/format";
import { useDismiss, useNow } from "../../shared/hooks";
import { StatusIcon } from "../../shared/ui/StatusIcon";

const t = vi.workspace.background;
/** A command's output is read again this often while it is shown. */
const OUTPUT_POLL_MS = 2000;

interface Props {
  sessionId: string;
  tasks: BackgroundTask[];
  /** stop the turn and every background task */
  onStopAll: () => void;
  /** land on a subagent's Agent call in the chat */
  onShowAgent: (toolUseId: string) => void;
  onSession: (s: SessionView) => void;
  disabled: boolean;
}

/** What the session's CLI runs in the background (subagents, commands run in the background),
 *  like the extension's list: what each does and for how long, its steps or output, Dừng. */
export function BackgroundTasks({ sessionId, tasks, onStopAll, onShowAgent, onSession, disabled }: Props) {
  const [open, setOpen] = useState(false);
  const [shown, setShown] = useState<string>();
  const [failure, setFailure] = useState<string>();
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));
  const now = useNow(1000);
  // the list closes by itself once nothing runs in the background
  useEffect(() => {
    if (tasks.length === 0) setOpen(false);
  }, [tasks.length]);
  if (tasks.length === 0) return null;

  const stop = (task: BackgroundTask) => {
    setFailure(undefined);
    stopSessionTask(sessionId, task.id)
      .then(onSession)
      .catch((e: unknown) => setFailure(errorMessage(e)));
  };

  return (
    <div className="ws-bg" ref={ref}>
      <button className={`ws-bg-chip${open ? " is-open" : ""}`} aria-expanded={open} title={t.chipHint}
        onClick={() => setOpen((o) => !o)}>
        <StatusIcon state="running" size={11} />
        {t.chip(tasks.length)}
      </button>
      {open && (
        <div className="ws-bg-pop" role="dialog" aria-label={t.title}>
          <header className="ws-bg-head">
            <span>{t.title}</span>
            <button className="btn btn--sm btn--ghost" disabled={disabled} title={t.stopAllHint} onClick={onStopAll}>
              {t.stopAll}
            </button>
          </header>
          {failure && <p className="note note--error">{failure}</p>}
          <ul className="ws-bg-list">
            {tasks.map((task) => (
              <li key={task.id} className="ws-bg-row">
                <div className="ws-bg-line">
                  <span className={`chip ws-bg-kind ws-bg-kind--${task.kind}`}>{t.kind[task.kind] ?? task.kind}</span>
                  <span className="ws-bg-what" title={task.description}>{task.description}</span>
                  <span className="faint ws-bg-time">{formatDuration(now - Date.parse(task.startedAt))}</span>
                </div>
                {(task.activity || task.toolUses !== undefined || task.tokens !== undefined) && (
                  <p className="faint ws-bg-activity">
                    {[
                      task.activity,
                      task.toolUses !== undefined ? t.toolUses(task.toolUses) : undefined,
                      task.tokens !== undefined ? t.tokens(formatTokens(task.tokens)) : undefined,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </p>
                )}
                <div className="ws-bg-actions">
                  {task.kind === "agent" && task.toolUseId ? (
                    <button className="btn btn--sm" title={t.showStepsHint} onClick={() => {
                      setOpen(false);
                      onShowAgent(task.toolUseId ?? "");
                    }}>
                      {t.showSteps}
                    </button>
                  ) : (
                    <button className="btn btn--sm" aria-pressed={shown === task.id} title={t.showOutputHint}
                      onClick={() => setShown((s) => (s === task.id ? undefined : task.id))}>
                      {shown === task.id ? t.hideOutput : t.showOutput}
                    </button>
                  )}
                  <button className="btn btn--sm btn--ghost" disabled={task.stopping} title={t.stopHint} onClick={() => stop(task)}>
                    {task.stopping ? t.stopping : t.stop}
                  </button>
                </div>
                {shown === task.id && <OutputView sessionId={sessionId} taskId={task.id} />}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/** The end of a command's output, read again while it is shown; stays at the bottom unless you
 *  scrolled up. */
function OutputView({ sessionId, taskId }: { sessionId: string; taskId: string }) {
  const [out, setOut] = useState<TaskOutput>();
  const [failure, setFailure] = useState<string>();
  const pre = useRef<HTMLPreElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    let alive = true;
    const read = () =>
      getTaskOutput(sessionId, taskId)
        .then((o) => alive && setOut(o))
        .catch((e: unknown) => alive && setFailure(errorMessage(e)));
    void read();
    const id = window.setInterval(() => void read(), OUTPUT_POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [sessionId, taskId]);
  useEffect(() => {
    const el = pre.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [out?.text]);
  if (failure) return <p className="note note--error">{failure}</p>;
  if (!out) return <p className="faint ws-bg-empty">{t.reading}</p>;
  if (!out.text.trim()) return <p className="faint ws-bg-empty">{out.file ? t.noOutputYet : t.noOutputFile}</p>;
  return (
    <pre ref={pre} className="ws-bg-out" title={out.file}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
      }}>
      {out.cut > 0 ? `${t.cut(out.cut)}\n` : ""}
      {out.text}
    </pre>
  );
}
