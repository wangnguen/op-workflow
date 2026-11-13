import { useState } from "react";
import { vi } from "../../i18n/vi";
import type { BoardSnapshot, TaskCard, TaskColumn } from "../../ipc/types";
import { StartWorkDialog } from "./StartWorkDialog";
import { TaskCardView } from "./TaskCardView";
import "./board.css";

interface Props {
  board: BoardSnapshot;
  onSettings: () => void;
}

const COLUMNS: { key: Exclude<TaskColumn, "done">; title: string; hint: string }[] = [
  { key: "inbox", title: vi.board.inbox, hint: vi.board.inboxHint },
  { key: "active", title: vi.board.active, hint: vi.board.activeHint },
  { key: "awaitingPush", title: vi.board.awaitingPush, hint: vi.board.awaitingPushHint },
];

/** Inbox / Đang làm (max 1) / Chờ bạn push. Only one task can be in progress. */
export function TaskBoard({ board, onSettings }: Props) {
  const [starting, setStarting] = useState<TaskCard | null>(null);
  const locked = board.activeTaskId !== undefined;

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
      <div className="board">
        {COLUMNS.map((col) => {
          const cards = board.tasks.filter((t) => t.column === col.key);
          const counter = col.key === "active" ? `${cards.length}/1` : String(cards.length);
          return (
            <section key={col.key} className={`column column--${col.key}`}>
              <header className="column-head">
                <h2>
                  {col.title} <span className="column-count">{counter}</span>
                </h2>
                <span className="faint">{col.hint}</span>
              </header>
              <div className="column-body">
                {cards.length === 0 && <div className="column-empty">{vi.board.empty}</div>}
                {cards.map((card) => (
                  <TaskCardView key={card.id} card={card} locked={locked} onStart={setStarting} />
                ))}
              </div>
            </section>
          );
        })}
        {starting && <StartWorkDialog task={starting} onClose={() => setStarting(null)} />}
      </div>
    </div>
  );
}
