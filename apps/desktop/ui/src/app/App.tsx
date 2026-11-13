import { useCallback, useState } from "react";
import { POLL_MS, getAppInfo, getBoard, getOrbit, isMockMode } from "../ipc/client";
import { TaskBoard } from "../features/board/TaskBoard";
import { SessionsView } from "../features/sessions/SessionsView";
import { SettingsDialog } from "../features/settings/SettingsDialog";
import { WorkspacePlaceholder } from "../features/workspace/WorkspacePlaceholder";
import { useNow, usePolling } from "../shared/hooks";
import { BottomBar } from "./BottomBar";
import { TopBar, type Tab } from "./TopBar";
import "./app.css";

const INFO_POLL_MS = 60_000;

export function App() {
  const [tab, setTab] = useState<Tab>("task");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const board = usePolling(getBoard, POLL_MS);
  const orbit = usePolling(getOrbit, POLL_MS);
  const info = usePolling(getAppInfo, INFO_POLL_MS);
  const now = useNow(1000);
  const { refresh: refreshBoard } = board;
  const { refresh: refreshOrbit } = orbit;
  const refreshAll = useCallback(() => {
    refreshBoard();
    refreshOrbit();
  }, [refreshBoard, refreshOrbit]);

  return (
    <div className="app">
      <TopBar
        tab={tab}
        onTab={setTab}
        quota={board.data?.quota ?? []}
        now={now}
        demo={isMockMode() || board.data?.mode === "demo"}
        needsYou={orbit.data?.active?.gate !== undefined}
        onRefresh={refreshAll}
        onSettings={() => setSettingsOpen(true)}
      />
      <main className="app-main">
        {tab === "task" && board.data && <TaskBoard board={board.data} onSettings={() => setSettingsOpen(true)} />}
        {tab === "workspace" && <WorkspacePlaceholder />}
        {tab === "sessions" && <SessionsView snapshot={orbit.data} now={now} />}
      </main>
      {settingsOpen && <SettingsDialog now={now} onClose={() => setSettingsOpen(false)} onChanged={refreshAll} />}
      <BottomBar
        board={board.data}
        orbit={orbit.data}
        version={info.data?.version}
        error={board.error ?? orbit.error}
      />
    </div>
  );
}
