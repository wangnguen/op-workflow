import { useState } from "react";
import { vi } from "../../i18n/vi";
import type { SessionState, SessionView, WorkspaceSnapshot } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { OTHER_PROJECT, type Selection, type TaskNode, buildTree, repoName, taskState } from "./chatState";

const w = vi.workspace;

interface Props {
  snapshot: WorkspaceSnapshot;
  selected: Selection | undefined;
  onSelect: (s: Selection) => void;
  onNew: () => void;
}

function Dot({ state }: { state: SessionState | undefined }) {
  return <span className={`ws-dot ws-dot--${state ?? "none"}`} aria-hidden="true" />;
}

function stateLabel(s: SessionView): string {
  return s.state === "outside" && s.outside ? w.outsideAt(s.outside) : (w.state[s.state] ?? s.state);
}

/** Project → task → sessions, then the sessions without a task. Every row opens its chat. */
export function SessionTree({ snapshot, selected, onSelect, onNew }: Props) {
  const tree = buildTree(snapshot);
  const [closed, setClosed] = useState<Record<string, boolean>>({});
  const toggle = (key: string) => setClosed((c) => ({ ...c, [key]: !c[key] }));
  const isOn = (s: Selection) =>
    selected !== undefined &&
    selected.kind === s.kind &&
    (s.kind === "session" ? selected.kind === "session" && selected.id === s.id : selected.kind === "task" && selected.wpId === s.wpId);

  const sessionRow = (s: SessionView, nested: boolean) => (
    <button
      key={s.id}
      className={`ws-row ws-row--session${nested ? " is-nested" : ""}${isOn({ kind: "session", id: s.id }) ? " is-on" : ""}`}
      onClick={() => onSelect({ kind: "session", id: s.id })}
      title={`${s.title} · ${stateLabel(s)}`}
    >
      <Dot state={s.state} />
      <span className="ws-row-main">
        <span className="ws-row-title">{s.title}</span>
        <span className="ws-row-meta faint">
          {stateLabel(s)}
          {!nested && ` · ${repoName(s.repo)}`}
          {` · ${formatHourMinute(s.updatedAt)}`}
        </span>
      </span>
    </button>
  );

  const taskRow = (n: TaskNode) => {
    const key = `task-${n.wpId}`;
    const open = !closed[key];
    const target: Selection = n.sessions[0] ? { kind: "session", id: n.sessions[0].id } : { kind: "task", wpId: n.wpId };
    const on = isOn({ kind: "task", wpId: n.wpId });
    return (
      <div key={key} className="ws-task">
        <div className={`ws-row ws-row--task${on ? " is-on" : ""}`}>
          <button
            className="ws-twist"
            aria-label={open ? w.collapse : w.expand}
            aria-expanded={open}
            disabled={n.sessions.length === 0}
            onClick={() => toggle(key)}
          >
            <Icon name="chevron" size={11} className={open && n.sessions.length > 0 ? "is-open" : undefined} />
          </button>
          <button className="ws-row-pick" onClick={() => onSelect(target)} title={n.title || n.taskId}>
            <Dot state={taskState(n)} />
            <span className="ws-row-main">
              <span className="ws-row-title">
                <span className="ws-task-id">{n.taskId}</span> {n.title}
              </span>
              <span className="ws-row-meta faint">
                {n.sessions.length === 0 ? w.noSession : `${n.sessions.length} phiên`}
                {n.repo && ` · ${repoName(n.repo)}`}
              </span>
            </span>
          </button>
        </div>
        {open && n.sessions.map((s) => sessionRow(s, true))}
      </div>
    );
  };

  const empty = tree.projects.length === 0 && tree.free.length === 0;
  return (
    <nav className="ws-tree" aria-label={w.title}>
      <div className="ws-tree-top">
        <button className="btn btn--sm ws-new" title={w.newSessionHint} onClick={onNew}>
          <span className="ws-plus" aria-hidden="true">
            +
          </span>
          {w.newSession}
        </button>
      </div>
      <div className="ws-tree-scroll">
        {empty && <p className="faint ws-tree-empty">{w.empty}</p>}
        {tree.projects.map((p) => {
          const key = `project-${p.project}`;
          const open = !closed[key];
          return (
            <section key={key} className="ws-group">
              <button className="ws-group-head" aria-expanded={open} onClick={() => toggle(key)}>
                <Icon name="chevron" size={11} className={open ? "is-open" : undefined} />
                <span>{p.project === OTHER_PROJECT ? vi.workspace.tasks : p.project}</span>
              </button>
              {open && p.tasks.map(taskRow)}
            </section>
          );
        })}
        {tree.free.length > 0 && (
          <section className="ws-group">
            <button className="ws-group-head" aria-expanded={!closed.free} onClick={() => toggle("free")}>
              <Icon name="chevron" size={11} className={!closed.free ? "is-open" : undefined} />
              <span>{w.free}</span>
            </button>
            {!closed.free && tree.free.map((s) => sessionRow(s, false))}
          </section>
        )}
      </div>
    </nav>
  );
}
