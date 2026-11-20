import { useCallback, useEffect, useRef, useState } from "react";
import {
  POLL_MS,
  getAppInfo,
  getBoard,
  getOrbit,
  getUpdateStatus,
  getWorkspace,
  isMockMode,
  markAlertsRead,
  notifySessionWait,
  onAlertClicked,
  onCloseAsked,
  onSessionClicked,
  openReleasePage,
} from "../ipc/client";
import { OPEN_QUOTA_EVENT } from "../features/quota/quota";
import { TaskPeek, type PeekTab } from "../features/board/TaskPeek";
import { TaskBoard } from "../features/board/TaskBoard";
import { SessionsView } from "../features/sessions/SessionsView";
import { SettingsDialog } from "../features/settings/SettingsDialog";
import { pickFor, type Selection, type WorkspaceTarget } from "../features/workspace/chatState";
import { WorkspaceView } from "../features/workspace/WorkspaceView";
import { TaskWindow } from "../features/task/TaskWindow";
import { asTab, type TaskTab } from "../features/task/taskTabs";
import { useNow, usePolling } from "../shared/hooks";
import { BottomBar } from "./BottomBar";
import { CloseDialog } from "./CloseDialog";
import { TopBar, type Tab } from "./TopBar";
import "./app.css";

const INFO_POLL_MS = 60_000;
/** Reads the cached result of the background check; the check itself runs every few hours. */
const UPDATE_POLL_MS = 60_000;

export function App() {
  const [tab, setTab] = useState<Tab>("task");
  const [settingsOpen, setSettingsOpen] = useState(false);
  // a task opened from the bell
  const [opened, setOpened] = useState<{ wpId: number; tab?: TaskTab }>();
  // the comments of an Inbox task, opened from the bell (it has no task window yet)
  const [peeking, setPeeking] = useState<{ wpId: number; taskId: string; title: string; tab: PeekTab }>();
  // the window's X asked what to do (`onClose` = ask): AI steps running at that moment
  const [closing, setClosing] = useState<number>();
  // the chat picked in Workspace, kept while other tabs are shown
  const [wsPicked, setWsPicked] = useState<Selection>();
  const board = usePolling(getBoard, POLL_MS);
  const workspace = usePolling(getWorkspace, POLL_MS);
  const orbit = usePolling(getOrbit, POLL_MS);
  const info = usePolling(getAppInfo, INFO_POLL_MS);
  const update = usePolling(getUpdateStatus, UPDATE_POLL_MS);
  const now = useNow(1000);
  const { refresh: refreshBoard } = board;
  const { refresh: refreshOrbit } = orbit;
  const { refresh: refreshWorkspace } = workspace;
  const refreshAll = useCallback(() => {
    refreshBoard();
    refreshOrbit();
    refreshWorkspace();
  }, [refreshBoard, refreshOrbit, refreshWorkspace]);
  const openRelease = useCallback(() => void openReleasePage().catch(() => setSettingsOpen(true)), []);
  /** A task from the bell or a clicked notification: its window, or its "Xem task" box while it
   *  is still in the Inbox (no task window yet). */
  const openTask = (wpId: number, alertTab: string | undefined) => {
    const known = board.data?.tasks.find((c) => c.wpId === wpId);
    if (known && known.column !== "inbox") setOpened({ wpId, tab: asTab(alertTab) });
    else {
      // inbox tasks have no task window yet: the board, with the task's box on top
      setTab("task");
      // the bell's rows of an Inbox task are its comments and status: the comments first
      const tab: PeekTab = alertTab === "description" ? "description" : "comments";
      if (known) setPeeking({ wpId, taskId: known.id, title: known.title, tab });
    }
  };
  // the latest board, for a notification clicked whenever
  const openTaskRef = useRef(openTask);
  openTaskRef.current = openTask;
  useEffect(
    () =>
      onAlertClicked((a) => {
        void markAlertsRead([a.id]).then(refreshAll);
        if (a.kind === "quota") window.dispatchEvent(new Event(OPEN_QUOTA_EVENT));
        else if (a.wpId !== undefined) openTaskRef.current(a.wpId, a.tab);
      }),
    [refreshAll],
  );
  useEffect(() => onCloseAsked(setClosing), []);

  // a session that starts waiting (permission, question, plan) while you look at another tab,
  // another chat or another window: a Windows notification; a click opens that session
  const waiting = useRef<Set<string>>(undefined);
  const looking = useRef({ tab, wsPicked });
  looking.current = { tab, wsPicked };
  useEffect(() => {
    const sessions = workspace.data?.sessions;
    if (!sessions) return;
    const now = new Set(sessions.filter((s) => s.state === "waiting").map((s) => s.id));
    const before = waiting.current;
    waiting.current = now;
    // the first read only learns what already waits
    if (!before) return;
    const { tab: shownTab, wsPicked: picked } = looking.current;
    const front = !document.hidden && document.hasFocus();
    for (const s of sessions) {
      if (!now.has(s.id) || before.has(s.id)) continue;
      const shown = front && shownTab === "workspace" &&
        ((picked?.kind === "session" && picked.id === s.id) || (picked?.kind === "task" && picked.wpId === s.wpId));
      if (!shown) void notifySessionWait(s.id).catch(() => undefined);
    }
  }, [workspace.data]);
  // the latest Workspace list, for a target sent whenever
  const wsData = useRef(workspace.data);
  wsData.current = workspace.data;
  /** Another screen sends you to Workspace: a session, a task's session (at a step, a tab or
   *  its question) or a session opened outside the app. A task window or box open here closes.
   *  `false` = Workspace has nothing for it (a finished task without a session). */
  const openWorkspace = useCallback((target: WorkspaceTarget): boolean => {
    const pick = pickFor(target, wsData.current, Date.now());
    if (!pick) return false;
    setOpened(undefined);
    setPeeking(undefined);
    setTab("workspace");
    setWsPicked(pick);
    return true;
  }, []);
  useEffect(() => onSessionClicked((id) => openWorkspace({ sessionId: id })), [openWorkspace]);

  return (
    <div className="app">
      <TopBar
        tab={tab}
        onTab={setTab}
        quota={board.data?.quota ?? []}
        links={board.data?.links ?? []}
        now={now}
        demo={isMockMode() || board.data?.mode === "demo"}
        needsYou={[orbit.data?.active, ...(orbit.data?.others ?? [])].some((s) => s?.gate !== undefined)}
        workspaceNeedsYou={workspace.data?.sessions.some((s) => s.state === "waiting") ?? false}
        unreadAlerts={board.data?.unreadAlerts ?? 0}
        onOpenTask={openTask}
        update={update.data}
        onRefresh={refreshAll}
        onSettings={() => setSettingsOpen(true)}
        onOpenRelease={openRelease}
        frameless={!isMockMode()}
      />
      <main className="app-main">
        {/* keyed by tab: the new screen fades in instead of popping */}
        <div className="tab-pane" key={tab}>
          {tab === "task" && board.data && (
            <TaskBoard board={board.data} onSettings={() => setSettingsOpen(true)} onChanged={refreshAll}
              onOpenWorkspace={openWorkspace} />
          )}
          {tab === "workspace" && (
            <WorkspaceView
              snapshot={workspace.data}
              selected={wsPicked}
              onSelect={setWsPicked}
              onChanged={refreshAll}
              onOpenTask={openTask}
            />
          )}
          {tab === "sessions" && (
            <SessionsView snapshot={orbit.data} board={board.data} workspace={workspace.data} now={now}
              onChanged={refreshAll} onOpenWorkspace={openWorkspace} />
          )}
        </div>
      </main>
      {opened && (
        <TaskWindow wpId={opened.wpId} tab={opened.tab} onClose={() => setOpened(undefined)} onChanged={refreshAll}
          onOpenWorkspace={openWorkspace} />
      )}
      {peeking && (
        <TaskPeek
          wpId={peeking.wpId}
          taskId={peeking.taskId}
          title={peeking.title}
          tab={peeking.tab}
          onClose={() => setPeeking(undefined)}
          onChanged={refreshAll}
        />
      )}
      {settingsOpen && (
        <SettingsDialog
          now={now}
          links={board.data?.links ?? []}
          onClose={() => setSettingsOpen(false)}
          onChanged={refreshAll}
          onUpdateChanged={update.refresh}
        />
      )}
      {closing !== undefined && <CloseDialog running={closing} onClose={() => setClosing(undefined)} />}
      <BottomBar
        board={board.data}
        orbit={orbit.data}
        version={info.data?.version}
        error={board.error ?? orbit.error}
      />
    </div>
  );
}
