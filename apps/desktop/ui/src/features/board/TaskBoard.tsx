import { useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, isMockMode, openBrief, pauseTask, setTasksDir } from "../../ipc/client";
import type { BoardSnapshot, TaskColumn } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { TaskCardView } from "./TaskCardView";
import { joined, useStartWork } from "./useStartWork";
import "./board.css";

interface Props {
  board: BoardSnapshot;
  onSettings: () => void;
  /** reload board + orbit after the user started or paused a task */
  onChanged: () => void;
}

const b = vi.board;

const COLUMNS: { key: Exclude<TaskColumn, "done">; title: string; hint: string }[] = [
  { key: "inbox", title: b.inbox, hint: b.inboxHint },
  { key: "active", title: b.active, hint: b.activeHint },
  { key: "awaitingPush", title: b.awaitingPush, hint: b.awaitingPushHint },
];

/** Inbox / Đang làm / Chờ bạn push. Several tasks can be in progress at once. */
export function TaskBoard({ board, onSettings, onChanged }: Props) {
  const start = useStartWork({ tasks: board.tasks, onChanged });
  const [offerBusy, setOfferBusy] = useState(false);

  const run = (action: Promise<unknown>) => {
    action.then(onChanged).catch((e: unknown) => start.setFlash({ kind: "warn", text: errorMessage(e) }));
  };

  const chooseTasksDir = (dir?: string) => {
    setOfferBusy(true);
    setTasksDir(dir)
      .then((r) => {
        const text = joined(vi.settings.tasksDirMoved(r.path, r.moved), ...r.warnings);
        start.setFlash({ kind: r.warnings.length > 0 ? "warn" : "ok", text });
        onChanged();
      })
      .catch((e: unknown) => start.setFlash({ kind: "warn", text: errorMessage(e) }))
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
      {start.banner}
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
                    onStart={start.open}
                    onPause={(c) => run(pauseTask(c.wpId))}
                    onBrief={isMockMode() ? undefined : (c) => run(openBrief(c.wpId))}
                    onIssues={start.readIssues}
                    readingIssues={start.reading.has(card.wpId)}
                  />
                ))}
              </div>
            </section>
          );
        })}
      </div>
      {start.dialog}
    </div>
  );
}
