import { DndContext, DragOverlay, type Announcements, type UniqueIdentifier } from "@dnd-kit/core";
import { useEffect, useRef, useState } from "react";
import { stepLabel, vi } from "../../i18n/vi";
import { errorMessage, flowAction, isMockMode, openBrief, openLogin, pauseTask } from "../../ipc/client";
import type { BoardSnapshot, TaskCard, TaskColumn } from "../../ipc/types";
import { ContinueDialog } from "../task/ContinueDialog";
import { TaskPeek, type PeekTab } from "./TaskPeek";
import { FinishDialog } from "../task/FinishDialog";
import { TaskWindow } from "../task/TaskWindow";
import type { TaskTab } from "../task/taskTabs";
import type { WorkspaceTarget } from "../workspace/chatState";
import { Icon } from "../../shared/ui/Icon";
import { dropRule, type DropAction } from "./dropRule";
import { DraggableCard, DropColumn, cardCollision, makeDropAnimation, useCardSensors, type DropEnd } from "./cardDnd";
import { TaskCardView } from "./TaskCardView";
import { useStartWork } from "./useStartWork";
import "./board.css";

interface Props {
  board: BoardSnapshot;
  onSettings: () => void;
  /** reload board + orbit after the user started or paused a task */
  onChanged: () => void;
  /** a task's chat in Workspace (`false`: Workspace has nothing for it) */
  onOpenWorkspace?: (target: WorkspaceTarget) => boolean;
}

const b = vi.board;
const noop = () => undefined;

const COLUMNS: { key: TaskColumn; title: string; hint: string }[] = [
  { key: "inbox", title: b.inbox, hint: b.inboxHint },
  { key: "active", title: b.active, hint: b.activeHint },
  { key: "awaitingPush", title: b.awaitingPush, hint: b.awaitingPushHint },
  { key: "done", title: b.done, hint: b.doneHint },
];

/** "Đã xong" open or folded, per machine. */
const DONE_OPEN_KEY = "opw.board.doneOpen";
const TOAST_MS = 4500;

function readDoneOpen(): boolean {
  try {
    return window.localStorage.getItem(DONE_OPEN_KEY) === "1";
  } catch {
    return false;
  }
}

interface Toast {
  kind: "ok" | "warn";
  text: string;
  /** a new toast with the same text restarts the timer */
  at: number;
  /** fading out: the text stays until the next one */
  hidden?: boolean;
}

/** Inbox / Đang làm / Chờ bạn push / Đã xong. Several tasks can be in progress at once; cards
 *  move between columns by drag and drop or with their buttons. */
export function TaskBoard({ board, onSettings, onChanged, onOpenWorkspace }: Props) {
  const start = useStartWork({ tasks: board.tasks, onChanged });
  const [opened, setOpened] = useState<{ wpId: number; tab?: TaskTab }>();
  // "Xem task" / "N comment mới" on an Inbox card
  const [peeking, setPeeking] = useState<{ card: TaskCard; tab: PeekTab }>();
  const [finishing, setFinishing] = useState<TaskCard>();
  const [continuing, setContinuing] = useState<{ card: TaskCard; to?: TaskColumn }>();
  const [doneOpen, setDoneOpen] = useState(readDoneOpen);
  const [toast, setToast] = useState<Toast>();
  const toastTimer = useRef(0);

  const say = (kind: Toast["kind"], text: string) => setToast({ kind, text, at: Date.now() });
  useEffect(() => {
    if (!toast || toast.hidden) return;
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast((t) => t && { ...t, hidden: true }), TOAST_MS);
    return () => window.clearTimeout(toastTimer.current);
  }, [toast]);

  const toggleDone = () =>
    setDoneOpen((open) => {
      try {
        window.localStorage.setItem(DONE_OPEN_KEY, open ? "0" : "1");
      } catch {
        // private mode: the column just starts folded next time
      }
      return !open;
    });

  const run = (action: Promise<unknown>) => {
    action.then(onChanged).catch((e: unknown) => start.setFlash({ kind: "warn", text: errorMessage(e) }));
  };

  const pause = (card: TaskCard): Promise<boolean> =>
    pauseTask(card.wpId)
      .then(() => {
        onChanged();
        say("ok", b.paused(card.id));
        return true;
      })
      .catch((e: unknown) => {
        say("warn", errorMessage(e));
        return false;
      });

  const dropped = (card: TaskCard, to: TaskColumn, action: DropAction): void | Promise<boolean> => {
    switch (action.kind) {
      case "start":
        start.open(card);
        return;
      case "pause":
        return pause(card);
      case "finish":
        setFinishing(card);
        return;
      case "continue":
        setContinuing({ card, to });
        return;
      default:
        return;
    }
  };
  // ---------- drag and drop (dnd-kit) ----------
  const sensors = useCardSensors();
  const [drag, setDrag] = useState<{ wpId: number; over?: TaskColumn } | null>(null);
  const dragged = drag ? board.tasks.find((t) => t.wpId === drag.wpId) : undefined;
  // what the copy of the card does after the drop (read by the drop animation)
  const pending = useRef<Promise<DropEnd>>(Promise.resolve("back"));
  const [dropAnimation] = useState(() => makeDropAnimation(() => pending.current));
  const endDrag = () => {
    setDrag(null);
    document.body.classList.remove("is-card-dragging");
  };
  /** Carry out a drop: refused or nowhere → back; a dialog → fade; pausing → wait for it. */
  const settle = (card: TaskCard | undefined, to: TaskColumn | undefined): Promise<DropEnd> => {
    if (!card || !to) return Promise.resolve("back");
    const action = dropRule(card, to);
    if (action.kind === "none") return Promise.resolve("back");
    if (action.kind === "refuse") {
      say("warn", action.reason);
      return Promise.resolve("back");
    }
    const answer = dropped(card, to, action);
    return answer ? answer.then((ok): DropEnd => (ok ? "fade" : "back"), (): DropEnd => "back") : Promise.resolve("fade");
  };
  const columnTitle = (id: UniqueIdentifier | undefined) => COLUMNS.find((c) => c.key === id)?.title ?? "";
  const cardName = (id: UniqueIdentifier) => board.tasks.find((t) => t.wpId === id)?.id ?? String(id);
  const announcements: Announcements = {
    onDragStart: ({ active }) => b.drag.lifted(cardName(active.id)),
    onDragOver: ({ active, over }) => (over ? b.drag.over(cardName(active.id), columnTitle(over.id)) : b.drag.outside),
    onDragEnd: ({ active, over }) =>
      over ? b.drag.dropped(cardName(active.id), columnTitle(over.id)) : b.drag.back(cardName(active.id)),
    onDragCancel: ({ active }) => b.drag.back(cardName(active.id)),
  };

  /** Drop highlight of a column while a card is dragged. */
  const dropClass = (col: TaskColumn): string => {
    if (!dragged || col === dragged.column) return "";
    const kind = dropRule(dragged, col).kind;
    const over = drag?.over === col;
    if (kind === "refuse") return over ? " is-drop-no" : "";
    return over ? " is-drop-over" : " is-drop-ready";
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
      {board.mode === "noToken" ? (
        <div className="board-banner">
          <span>{board.notice ?? vi.app.noToken}</span>
          <button className="btn btn--sm btn--primary" onClick={onSettings}>
            {vi.app.openSettings}
          </button>
        </div>
      ) : (
        board.notice && <div className="board-banner board-banner--warn">{board.notice}</div>
      )}
      {start.banner}
      <DndContext
        sensors={sensors}
        collisionDetection={cardCollision}
        accessibility={{ announcements, screenReaderInstructions: { draggable: b.drag.instructions } }}
        onDragStart={({ active }) => {
          setDrag({ wpId: Number(active.id) });
          document.body.classList.add("is-card-dragging");
        }}
        onDragOver={({ over }) => setDrag((d) => d && { ...d, over: over?.id as TaskColumn | undefined })}
        onDragEnd={({ active, over }) => {
          endDrag();
          const card = board.tasks.find((t) => t.wpId === active.id);
          pending.current = settle(card, over?.id as TaskColumn | undefined);
        }}
        onDragCancel={() => {
          endDrag();
          pending.current = Promise.resolve("back");
        }}
      >
        <div className={`board${doneOpen ? " is-done-open" : ""}${dragged ? " is-dragging" : ""}`}>
          {COLUMNS.map((col) => {
            const cards = board.tasks.filter((t) => t.column === col.key);
            const folded = col.key === "done" && !doneOpen;
            return (
              <DropColumn
                key={col.key}
                id={col.key}
                className={`column column--${col.key}${folded ? " is-folded" : ""}${dropClass(col.key)}`}
                onClick={folded ? toggleDone : undefined}
              >
                <header className="column-head">
                  {/* the done column's title fades in again when it folds or opens */}
                  <h2 key={folded ? "folded" : "open"}>
                    <span className="column-title">{col.title}</span> <span className="column-count">{cards.length}</span>
                  </h2>
                  {!folded && <span className="faint column-hint">{col.hint}</span>}
                  {col.key === "done" && (
                    <button
                      className="btn btn--ghost btn--icon btn--sm column-fold"
                      title={doneOpen ? b.doneHide : b.doneShow}
                      aria-label={doneOpen ? b.doneHide : b.doneShow}
                      aria-expanded={doneOpen}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleDone();
                      }}
                    >
                      <Icon name="chevron" size={14} />
                    </button>
                  )}
                </header>
                {!folded && (
                  <div className="column-body">
                    {cards.length === 0 && <div className="column-empty">{b.empty}</div>}
                    {cards.map((card) => (
                      <DraggableCard
                        key={card.id}
                        card={card}
                        onStart={start.open}
                        onPause={(c) => run(pauseTask(c.wpId))}
                        onBrief={isMockMode() ? undefined : (c) => run(openBrief(c.wpId))}
                        onIssues={start.readIssues}
                        readingIssues={start.reading.has(card.wpId)}
                        onOpen={(c, tab) => setOpened({ wpId: c.wpId, tab })}
                        onFlow={(c, action) => run(flowAction(c.wpId, action))}
                        onFinish={setFinishing}
                        onContinue={(c) => setContinuing({ card: c })}
                        onPeek={(c, tab) => setPeeking({ card: c, tab })}
                        onSession={(c) => {
                          // nothing left in Workspace (a finished task): its window instead
                          if (!onOpenWorkspace?.({ wpId: c.wpId })) setOpened({ wpId: c.wpId });
                        }}
                        onLogin={(p) => run(openLogin(p))}
                        onError={(text) => start.setFlash({ kind: "warn", text })}
                      />
                    ))}
                  </div>
                )}
              </DropColumn>
            );
          })}
        </div>
        <DragOverlay dropAnimation={dropAnimation} zIndex={70}>
          {dragged && (
            // a still copy: its buttons do nothing
            <div className="card-lift" inert>
              <TaskCardView
                card={dragged}
                onStart={noop}
                onPause={noop}
                onOpen={noop}
                onFlow={noop}
                onFinish={noop}
                onContinue={noop}
              />
            </div>
          )}
        </DragOverlay>
      </DndContext>
      <div
        className={`drop-toast${toast && !toast.hidden ? " is-shown" : ""}${toast?.kind === "warn" ? " drop-toast--warn" : ""}`}
        role="status"
      >
        {toast && (
          <>
            <Icon name={toast.kind === "warn" ? "alert" : "check"} size={14} />
            <span>{toast.text}</span>
          </>
        )}
      </div>
      {start.dialog}
      {finishing && (
        <FinishDialog
          wpId={finishing.wpId}
          taskId={finishing.id}
          branch={finishing.branch}
          atPush={finishing.column === "awaitingPush"}
          stepName={finishing.step ? stepLabel[finishing.step.kind] : undefined}
          onClose={() => setFinishing(undefined)}
          onFinished={(_, pushed) => {
            onChanged();
            if (!pushed) say("ok", b.movedDone(finishing.id));
          }}
        />
      )}
      {continuing && (
        <ContinueDialog
          wpId={continuing.card.wpId}
          taskId={continuing.card.id}
          to={continuing.to}
          onClose={() => setContinuing(undefined)}
          onDone={(_, step) => {
            setContinuing(undefined);
            onChanged();
            say("ok", b.reopened(continuing.card.id, stepLabel[step ?? "pushSummary"]));
          }}
        />
      )}
      {opened && (
        <TaskWindow wpId={opened.wpId} tab={opened.tab} onClose={() => setOpened(undefined)} onChanged={onChanged}
          onOpenWorkspace={onOpenWorkspace} />
      )}
      {peeking && (
        <TaskPeek
          wpId={peeking.card.wpId}
          taskId={peeking.card.id}
          title={peeking.card.title}
          tab={peeking.tab}
          onClose={() => setPeeking(undefined)}
          onChanged={onChanged}
          onStart={() => start.open(peeking.card)}
        />
      )}
    </div>
  );
}
