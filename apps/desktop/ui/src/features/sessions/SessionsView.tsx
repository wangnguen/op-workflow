import { useMemo, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, isMockMode, openBrief, openTaskLink } from "../../ipc/client";
import type { BoardSnapshot, OrbitSnapshot, TaskCard } from "../../ipc/types";
import { useDismiss } from "../../shared/hooks";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { StatusChip } from "../../shared/ui/StatusChip";
import { useStartWork } from "../board/useStartWork";
import "../board/board.css";
import { OrbitView } from "../orbit/OrbitView";
import { TaskWindow } from "../task/TaskWindow";
import type { TaskTab } from "../task/taskTabs";
import { SessionBoard } from "./SessionBoard";
import "./sessions.css";

interface Props {
  snapshot: OrbitSnapshot | undefined;
  /** task cards (Inbox to start from here, ids → wpId for brief / issues) */
  board: BoardSnapshot | undefined;
  now: number;
  /** reload board + orbit after starting a task */
  onChanged: () => void;
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

/** "Phiên": watch the agents on the map (Quỹ đạo) or as cards (Bảng); start tasks from here too. */
export function SessionsView({ snapshot, board, now, onChanged }: Props) {
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
  const reading = new Set(tasks.filter((t) => start.reading.has(t.wpId)).map((t) => t.id));
  const fail = (e: unknown) => start.setFlash({ kind: "warn", text: errorMessage(e) });
  const openTask = (taskId: string, tab?: TaskTab) => {
    const c = card(taskId);
    if (c) setOpened({ wpId: c.wpId, tab });
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
              />
            </div>
            <div {...pane("board")}>
              <SessionBoard
                snapshot={snapshot}
                onShow={(taskId) => {
                  setFocusId(taskId);
                  changeMode("orbit");
                }}
                onIssues={(taskId) => {
                  const c = card(taskId);
                  if (c) start.readIssues(c);
                }}
                onBrief={
                  isMockMode()
                    ? undefined
                    : (taskId) => {
                        const c = card(taskId);
                        if (c) openBrief(c.wpId).catch(fail);
                      }
                }
                reading={reading}
                onOpen={openTask}
                wpIdOf={(taskId) => card(taskId)?.wpId}
                onChanged={onChanged}
              />
            </div>
          </>
        )}
      </div>
      {start.dialog}
      {opened && (
        <TaskWindow wpId={opened.wpId} tab={opened.tab} onClose={() => setOpened(undefined)} onChanged={onChanged} />
      )}
    </div>
  );
}
