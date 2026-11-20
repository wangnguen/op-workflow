import { useState } from "react";
import { vi } from "../../i18n/vi";
import type { WorkspaceSnapshot } from "../../ipc/types";
import { ChatPane } from "./ChatPane";
import type { Selection } from "./chatState";
import { NewSessionDialog } from "./NewSessionDialog";
import { SessionTree } from "./SessionTree";
import "./workspace.css";

interface Props {
  snapshot: WorkspaceSnapshot | undefined;
  /** kept by the app, so switching tabs comes back to the same chat */
  selected: Selection | undefined;
  onSelect: (s: Selection | undefined) => void;
  onChanged: () => void;
}

/** Where the work happens (PLAN.md §5): the tree of tasks and sessions, and the chat of the one
 *  picked. */
export function WorkspaceView({ snapshot, selected, onSelect, onChanged }: Props) {
  const [creating, setCreating] = useState(false);
  const data = snapshot ?? { tasks: [], sessions: [] };
  const session = selected?.kind === "session" ? data.sessions.find((s) => s.id === selected.id) : undefined;
  const task =
    selected?.kind === "task"
      ? data.tasks.find((t) => t.wpId === selected.wpId)
      : session?.wpId !== undefined
        ? data.tasks.find((t) => t.wpId === session.wpId)
        : undefined;
  // a session id from a selection the snapshot does not list yet (just created) still opens
  const sessionId = selected?.kind === "session" ? selected.id : undefined;

  return (
    <div className="ws">
      <SessionTree snapshot={data} selected={selected} onSelect={onSelect} onNew={() => setCreating(true)} />
      <section className="ws-main">
        {sessionId || task ? (
          <ChatPane
            key={sessionId ?? `task-${task?.wpId}`}
            sessionId={sessionId}
            task={task}
            onCreated={(id) => {
              onSelect({ kind: "session", id });
              onChanged();
            }}
            onHidden={() => {
              onSelect(undefined);
              onChanged();
            }}
            onChanged={onChanged}
          />
        ) : (
          <div className="placeholder">
            <p className="dim">{vi.workspace.pick}</p>
          </div>
        )}
      </section>
      {creating && (
        <NewSessionDialog
          onClose={() => setCreating(false)}
          onCreated={(id) => {
            setCreating(false);
            onSelect({ kind: "session", id });
            onChanged();
          }}
        />
      )}
    </div>
  );
}
