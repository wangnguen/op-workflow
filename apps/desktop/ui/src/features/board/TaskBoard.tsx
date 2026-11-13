import { useEffect, useState } from "react";
import { vi } from "../../i18n/vi";
import {
  errorCode,
  errorMessage,
  fetchIssues,
  isMockMode,
  openBrief,
  openBrowserDebug,
  pauseTask,
  setTasksDir,
} from "../../ipc/client";
import type { BoardSnapshot, StartResult, TaskCard, TaskColumn } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { StartWorkDialog } from "./StartWorkDialog";
import { TaskCardView } from "./TaskCardView";
import "./board.css";

interface Props {
  board: BoardSnapshot;
  onSettings: () => void;
  /** reload board + orbit after the user started or paused a task */
  onChanged: () => void;
}

interface Flash {
  /** "busy" stays until replaced (Chrome may wait for the user's Allow) */
  kind: "ok" | "warn" | "busy";
  text: string;
  action?: { label: string; run: () => void };
}

const FLASH_MS = 8000;
const b = vi.board;

const COLUMNS: { key: Exclude<TaskColumn, "done">; title: string; hint: string }[] = [
  { key: "inbox", title: b.inbox, hint: b.inboxHint },
  { key: "active", title: b.active, hint: b.activeHint },
  { key: "awaitingPush", title: b.awaitingPush, hint: b.awaitingPushHint },
];

const joined = (...parts: (string | undefined)[]) => parts.filter(Boolean).join(". ");

/** Inbox / Đang làm / Chờ bạn push. Several tasks can be in progress at once. */
export function TaskBoard({ board, onSettings, onChanged }: Props) {
  const [starting, setStarting] = useState<TaskCard | null>(null);
  const [flash, setFlash] = useState<Flash | null>(null);
  const [reading, setReading] = useState<ReadonlySet<number>>(new Set());
  const [offerBusy, setOfferBusy] = useState(false);
  const inProgress = board.tasks.filter((t) => t.column === "active");

  useEffect(() => {
    if (!flash || flash.kind === "busy") return;
    const id = window.setTimeout(() => setFlash(null), FLASH_MS);
    return () => window.clearTimeout(id);
  }, [flash]);

  const fail = (e: unknown) => setFlash({ kind: "warn", text: errorMessage(e) });

  /** Read the task's GitHub issues through the user's Chrome into its brief. */
  const readIssues = (wpId: number, taskId: string, before?: string) => {
    setReading((r) => new Set(r).add(wpId));
    setFlash({ kind: "busy", text: joined(before, b.issuesReading(taskId)) });
    fetchIssues(wpId)
      .then((r) => {
        const done = b.issuesDone(taskId, r.read);
        setFlash(
          r.warnings.length > 0
            ? { kind: "warn", text: joined(before, done, r.warnings.join(" ")) }
            : { kind: "ok", text: joined(before, done) },
        );
      })
      .catch((e: unknown) => {
        const action =
          errorCode(e) === "browser.notEnabled"
            ? { label: b.issuesOpenSettings, run: () => void openBrowserDebug().catch(fail) }
            : undefined;
        setFlash({ kind: "warn", text: joined(before, `${taskId}: ${errorMessage(e)}`), action });
      })
      .finally(() =>
        setReading((r) => {
          const next = new Set(r);
          next.delete(wpId);
          return next;
        }),
      );
  };

  const onStarted = (r: StartResult) => {
    const wpId = starting?.wpId;
    setStarting(null);
    const done = joined(vi.start.done(r.taskId, r.branch, r.created), ...r.warnings);
    if (wpId !== undefined && (r.issueLinks ?? 0) > 0 && !isMockMode()) {
      readIssues(wpId, r.taskId, done);
    } else {
      setFlash({ kind: r.warnings.length > 0 ? "warn" : "ok", text: done });
    }
    onChanged();
  };

  const run = (action: Promise<unknown>) => {
    action.then(onChanged).catch(fail);
  };

  const chooseTasksDir = (dir?: string) => {
    setOfferBusy(true);
    setTasksDir(dir)
      .then((r) => {
        const text = joined(vi.settings.tasksDirMoved(r.path, r.moved), ...r.warnings);
        setFlash({ kind: r.warnings.length > 0 ? "warn" : "ok", text });
        onChanged();
      })
      .catch(fail)
      .finally(() => setOfferBusy(false));
  };

  return (
    <div className="board-wrap">
      {board.mode === "demo" && (
        <div className="board-banner">
          <span>{vi.app.demoBoard}</span>
          <button className="btn btn--sm btn--primary" onClick={onSettings}>
            {vi.app.openSettings}
          </button>
        </div>
      )}
      {board.notice && <div className="board-banner board-banner--warn">{board.notice}</div>}
      {board.tasksDirOffer && (
        <div className="board-banner board-banner--ask">
          <span className="board-banner-text">
            <Icon name="folder" size={14} />
            {b.tasksDirAsk(board.tasksDirOffer)}
          </span>
          <span className="board-banner-actions">
            <button className="btn btn--sm btn--ghost" disabled={offerBusy} onClick={() => chooseTasksDir(undefined)}>
              {b.tasksDirNo}
            </button>
            <button
              className="btn btn--sm btn--primary"
              disabled={offerBusy}
              onClick={() => chooseTasksDir(board.tasksDirOffer)}
            >
              {b.tasksDirYes}
            </button>
          </span>
        </div>
      )}
      {flash && (
        <div className={`board-banner board-banner--${flash.kind}`} role="status">
          <span className="board-banner-text">
            {flash.kind === "busy" && <Icon name="refresh" size={13} className="is-spinning" />}
            {flash.text}
          </span>
          <span className="board-banner-actions">
            {flash.action && (
              <button className="btn btn--sm" onClick={flash.action.run}>
                {flash.action.label}
              </button>
            )}
            <button className="btn btn--sm btn--ghost btn--icon" onClick={() => setFlash(null)} aria-label={vi.settings.close}>
              <Icon name="close" size={13} />
            </button>
          </span>
        </div>
      )}
      <div className="board">
        {COLUMNS.map((col) => {
          const cards = board.tasks.filter((t) => t.column === col.key);
          return (
            <section key={col.key} className={`column column--${col.key}`}>
              <header className="column-head">
                <h2>
                  {col.title} <span className="column-count">{cards.length}</span>
                </h2>
                <span className="faint">{col.hint}</span>
              </header>
              <div className="column-body">
                {cards.length === 0 && <div className="column-empty">{b.empty}</div>}
                {cards.map((card) => (
                  <TaskCardView
                    key={card.id}
                    card={card}
                    onStart={setStarting}
                    onPause={(c) => run(pauseTask(c.wpId))}
                    onBrief={isMockMode() ? undefined : (c) => run(openBrief(c.wpId))}
                    onIssues={(c) => readIssues(c.wpId, c.id)}
                    readingIssues={reading.has(card.wpId)}
                  />
                ))}
              </div>
            </section>
          );
        })}
      </div>
      {starting && (
        <StartWorkDialog
          task={starting}
          inProgress={inProgress.filter((t) => t.wpId !== starting.wpId)}
          onClose={() => setStarting(null)}
          onStarted={onStarted}
        />
      )}
    </div>
  );
}
