import { useCallback, useState } from "react";
import { POLL_MS, getAppInfo, getBoard, getOrbit, getUpdateStatus, isMockMode, openReleasePage } from "../ipc/client";
import { TaskBoard } from "../features/board/TaskBoard";
import { SessionsView } from "../features/sessions/SessionsView";
import { SettingsDialog } from "../features/settings/SettingsDialog";
import { WorkspacePlaceholder } from "../features/workspace/WorkspacePlaceholder";
import { TaskWindow } from "../features/task/TaskWindow";
import { asTab, type TaskTab } from "../features/task/taskTabs";
import { useNow, usePolling } from "../shared/hooks";
import { BottomBar } from "./BottomBar";
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
  const board = usePolling(getBoard, POLL_MS);
  const orbit = usePolling(getOrbit, POLL_MS);
  const info = usePolling(getAppInfo, INFO_POLL_MS);
  const update = usePolling(getUpdateStatus, UPDATE_POLL_MS);
  const now = useNow(1000);
  const { refresh: refreshBoard } = board;
  const { refresh: refreshOrbit } = orbit;
  const refreshAll = useCallback(() => {
    refreshBoard();
    refreshOrbit();
  }, [refreshBoard, refreshOrbit]);
  const openRelease = useCallback(() => void openReleasePage().catch(() => setSettingsOpen(true)), []);

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
        unreadAlerts={board.data?.unreadAlerts ?? 0}
        onOpenTask={(wpId, tab) => {
          const known = board.data?.tasks.find((c) => c.wpId === wpId);
          // inbox tasks have no task window yet: show the board instead
          if (known && known.column !== "inbox") setOpened({ wpId, tab: asTab(tab) });
          else setTab("task");
        }}
        update={update.data}
        onRefresh={refreshAll}
        onSettings={() => setSettingsOpen(true)}
        onOpenRelease={openRelease}
      />
      <main className="app-main">
        {/* keyed by tab: the new screen fades in instead of popping */}
        <div className="tab-pane" key={tab}>
          {tab === "task" && board.data && (
            <TaskBoard board={board.data} onSettings={() => setSettingsOpen(true)} onChanged={refreshAll} />
          )}
          {tab === "workspace" && <WorkspacePlaceholder />}
          {tab === "sessions" && (
            <SessionsView snapshot={orbit.data} board={board.data} now={now} onChanged={refreshAll} />
          )}
        </div>
      </main>
      {opened && (
        <TaskWindow wpId={opened.wpId} tab={opened.tab} onClose={() => setOpened(undefined)} onChanged={refreshAll} />
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
      <BottomBar
        board={board.data}
        orbit={orbit.data}
        version={info.data?.version}
        error={board.error ?? orbit.error}
      />
    </div>
  );
}
