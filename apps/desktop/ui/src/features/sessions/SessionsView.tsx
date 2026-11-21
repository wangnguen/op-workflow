import { useMemo, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, openTaskLink } from "../../ipc/client";
import type { BoardSnapshot, OrbitSnapshot, TaskCard, WorkspaceSnapshot } from "../../ipc/types";
import { useDismiss } from "../../shared/hooks";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { StatusChip } from "../../shared/ui/StatusChip";
import { useStartWork } from "../board/useStartWork";
import "../board/board.css";
import { OrbitView } from "../orbit/OrbitView";
import type { Go } from "../orbit/primary";
import type { WorkspaceTarget } from "../workspace/chatState";
import { TaskWindow } from "../task/TaskWindow";
import type { TaskTab } from "../task/taskTabs";
import { SessionBoard } from "./SessionBoard";
import "./sessions.css";

interface Props {
  snapshot: OrbitSnapshot | undefined;
  /** task cards (Inbox to start from here, ids → wpId for brief / issues) */
  board: BoardSnapshot | undefined;
  /** Workspace sessions: which task's session waits, and the cards of sessions without a task */
  workspace: WorkspaceSnapshot | undefined;
  now: number;
  /** reload board + orbit after starting a task */
  onChanged: () => void;
  /** go to a session in Workspace (`false`: Workspace has nothing for it) */
  onOpenWorkspace?: (target: WorkspaceTarget) => boolean;
}

type Mode = "orbit" | "board";
const MODE_KEY = "opw.sessions.mode";

function loadMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === "board" ? "board" : "orbit";
  } catch {
    return "orbit";
  }
}

/** "Bắt đầu task": pick an Inbox task and start it without leaving Phiên. */
function StartMenu({ inbox, disabled, onPick }: { inbox: TaskCard[]; disabled: boolean; onPick: (t: TaskCard) => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));
  return (
    <div className="s-start" ref={ref}>
      <button
        className="btn btn--primary btn--sm"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        title={vi.sessions.startHint}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="play" size={12} />
        {vi.sessions.start}
        {inbox.length > 0 && <span className="s-start-count">{inbox.length}</span>}
      </button>
      {open && (
        <div className="s-start-menu" role="menu" aria-label={vi.sessions.startPick}>
          <div className="s-start-head">{vi.sessions.startPick}</div>
          {inbox.length === 0 && <div className="s-start-empty">{vi.sessions.startEmpty}</div>}
          {inbox.map((t) => (
            <button
              key={t.id}
              role="menuitem"
              className="s-start-item"
              onClick={() => {
                setOpen(false);
                onPick(t);
              }}
            >
              <span className="s-start-line">
                <span className="mono s-start-id">{t.id}</span>
                <span className="faint s-start-project">{t.project}</span>
                <StatusChip name={t.nxStatus} color={t.nxStatusColor} />
              </span>
              <span className="s-start-title">{t.title}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** "Phiên": watch the agents on the map (Quỹ đạo) or in a table (Bảng); start tasks from here too. */
export function SessionsView({ snapshot, board, workspace, now, onChanged, onOpenWorkspace }: Props) {
  const [mode, setMode] = useState<Mode>(loadMode);
  // the task in the centre of the map; a task started here is centred right away
  const [focusId, setFocusId] = useState<string | null>(null);
  const tasks = useMemo(() => board?.tasks ?? [], [board]);
  const start = useStartWork({ tasks, onChanged, onStarted: setFocusId });
  const [opened, setOpened] = useState<{ wpId: number; tab?: TaskTab }>();

  const changeMode = (next: Mode) => {
    setMode(next);
    try {
      localStorage.setItem(MODE_KEY, next);
    } catch {
      // private window: the choice just is not remembered
    }
  };
  const card = (taskId: string) => tasks.find((t) => t.id === taskId);
  const inbox = tasks.filter((t) => t.column === "inbox");
  const activeCount = tasks.filter((t) => t.column === "active").length;
  const fail = (e: unknown) => start.setFlash({ kind: "warn", text: errorMessage(e) });
  const openTask = (taskId: string, tab?: TaskTab) => {
    const c = card(taskId);
    if (c) setOpened({ wpId: c.wpId, tab });
  };
  // tasks whose session waits for an answer: their gate on the map leads to the question
  const waiting = useMemo(
    () => new Set((workspace?.sessions ?? []).filter((s) => s.state === "waiting" && s.taskId).map((s) => s.taskId ?? "")),
    [workspace],
  );
  /** Where the map's main button, a double click or a card's "Tới phiên" leads. */
  const go = (g: Go) => {
    switch (g.to) {
      case "session": {
        const c = card(g.taskId);
        // Workspace has nothing for the task: its window, on the Chat tab
        if (!c || !onOpenWorkspace?.({ wpId: c.wpId, step: g.step, ask: g.ask })) openTask(g.taskId, "chat");
        return;
      }
      case "chat":
        onOpenWorkspace?.({ sessionId: g.sessionId, ask: g.ask });
        return;
      case "outside":
        onOpenWorkspace?.({ outsideId: g.id });
        return;
      case "task":
        openTask(g.taskId, g.tab);
        return;
      case "start": {
        const c = card(g.taskId);
        if (c) start.open(c);
      }
    }
  };

  // Both views stay mounted: switching only fades the other one in on top, so nothing is
  // rebuilt and the map keeps its zoom, position and selection.
  const pane = (which: Mode) => ({
    className: `sessions-pane sessions-pane--${which}${mode === which ? " is-shown" : ""}`,
    "aria-hidden": mode !== which,
    inert: mode !== which,
  });

  return (
    <div className="sessions">
      <header className="sessions-head">
        <SegmentedControl<Mode>
          label={vi.tabs.sessions}
          value={mode}
          onChange={changeMode}
          options={[
            { value: "orbit", label: vi.sessions.orbit, icon: <Icon name="dot" size={12} /> },
            { value: "board", label: vi.sessions.board, icon: <Icon name="layers" size={12} /> },
          ]}
        />
        {board && <span className="sessions-summary faint">{vi.sessions.summary(activeCount, inbox.length)}</span>}
        <StartMenu inbox={inbox} disabled={!board} onPick={start.open} />
      </header>
      <div className="sessions-body">
        {start.banner && <div className="sessions-banner">{start.banner}</div>}
        {snapshot && (
          <>
            <div {...pane("orbit")}>
              <OrbitView
                snapshot={snapshot}
                now={now}
                focusId={focusId}
                onFocus={setFocusId}
                paused={mode !== "orbit"}
                onStartTask={(taskId) => {
                  const c = card(taskId);
                  if (c) start.open(c);
                }}
                onOpenTask={openTask}
                onOpenLink={(wpId) => openTaskLink(wpId).catch(fail)}
                onGo={go}
                waiting={waiting}
              />
            </div>
            <div {...pane("board")}>
              <SessionBoard
                snapshot={snapshot}
                workspace={workspace}
                now={now}
                onGo={go}
                onOpen={openTask}
                onChanged={onChanged}
                onError={fail}
              />
            </div>
          </>
        )}
      </div>
      {start.dialog}
      {opened && (
        <TaskWindow wpId={opened.wpId} tab={opened.tab} onClose={() => setOpened(undefined)} onChanged={onChanged}
          onOpenWorkspace={onOpenWorkspace} />
      )}
    </div>
  );
}
