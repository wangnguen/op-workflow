import { useState } from "react";
import { vi } from "../../i18n/vi";
import type { OrbitSnapshot } from "../../ipc/types";
import { OrbitView } from "../orbit/OrbitView";
import { SessionBoard } from "./SessionBoard";
import "./sessions.css";

interface Props {
  snapshot: OrbitSnapshot | undefined;
  now: number;
}

type Mode = "orbit" | "board";

/** "Phiên": two ways to watch the agents, Quỹ đạo (map) and Bảng (cards). */
export function SessionsView({ snapshot, now }: Props) {
  const [mode, setMode] = useState<Mode>("orbit");
  return (
    <div className="sessions">
      <div className={`segmented segmented--${mode}`} role="tablist" aria-label={vi.tabs.sessions}>
        <button role="tab" aria-selected={mode === "orbit"} className={mode === "orbit" ? "is-on" : ""} onClick={() => setMode("orbit")}>
          ◎ {vi.sessions.orbit}
        </button>
        <button role="tab" aria-selected={mode === "board"} className={mode === "board" ? "is-on" : ""} onClick={() => setMode("board")}>
          ▦ {vi.sessions.board}
        </button>
      </div>
      <div className="sessions-body">
        {snapshot ? (
          mode === "orbit" ? (
            <OrbitView snapshot={snapshot} now={now} />
          ) : (
            <SessionBoard snapshot={snapshot} />
          )
        ) : null}
      </div>
    </div>
  );
}
