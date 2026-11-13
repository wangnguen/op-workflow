import { useCallback, useState } from "react";
import { POLL_MS, getAppInfo, getBoard, getOrbit, getUpdateStatus, isMockMode, openReleasePage } from "../ipc/client";
import { TaskBoard } from "../features/board/TaskBoard";
import { SessionsView } from "../features/sessions/SessionsView";
import { SettingsDialog } from "../features/settings/SettingsDialog";
import { WorkspacePlaceholder } from "../features/workspace/WorkspacePlaceholder";
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
        update={update.data}
        onRefresh={refreshAll}
        onSettings={() => setSettingsOpen(true)}
        onOpenRelease={openRelease}
      />
      <main className="app-main">
        {tab === "task" && board.data && (
          <TaskBoard board={board.data} onSettings={() => setSettingsOpen(true)} onChanged={refreshAll} />
        )}
        {tab === "workspace" && <WorkspacePlaceholder />}
        {tab === "sessions" && (
          <SessionsView snapshot={orbit.data} board={board.data} now={now} onChanged={refreshAll} />
        )}
      </main>
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
