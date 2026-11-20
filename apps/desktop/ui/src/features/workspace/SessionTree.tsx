import { useState } from "react";
import { vi } from "../../i18n/vi";
import type { OutsideSession, SessionState, SessionView, WorkspaceSnapshot } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { HideSideButton } from "../../shared/ui/SidePanel";
import { OTHER_PROJECT, type Selection, type TaskNode, buildTree, repoName, samePick, taskState } from "./chatState";

const w = vi.workspace;

interface Props {
  snapshot: WorkspaceSnapshot;
  selected: Selection | undefined;
  onSelect: (s: Selection) => void;
  onNew: () => void;
  /** hide the panel the list sits in */
  onHide: () => void;
}

function Dot({ state }: { state: SessionState | undefined }) {
  return <span className={`ws-dot ws-dot--${state ?? "none"}`} aria-hidden="true" />;
}

function stateLabel(s: SessionView): string {
  return s.state === "outside" && s.outside ? w.outsideAt(s.outside) : (w.state[s.state] ?? s.state);
}

/** Project → task → sessions, then the sessions without a task, then those open outside the
 *  app. Every row opens its chat. */
export function SessionTree({ snapshot, selected, onSelect, onNew, onHide }: Props) {
  const tree = buildTree(snapshot);
  const [closed, setClosed] = useState<Record<string, boolean>>({});
  const toggle = (key: string) => setClosed((c) => ({ ...c, [key]: !c[key] }));
  const isOn = (s: Selection) => samePick(selected, s);

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
    // the task's chat is shown (the task itself or one of its sessions)
    const shown =
      isOn({ kind: "task", wpId: n.wpId }) || n.sessions.some((s) => isOn({ kind: "session", id: s.id }));
    // folded with its shown session inside: the task row carries the mark
    const on = isOn({ kind: "task", wpId: n.wpId }) || (shown && !open);
    const folds = n.sessions.length > 0;
    return (
      <div key={key} className="ws-task">
        <div className={`ws-row ws-row--task${on ? " is-on" : ""}`}>
          <button
            className="ws-twist"
            aria-label={open ? w.collapse : w.expand}
            aria-expanded={open}
            disabled={!folds}
            onClick={() => toggle(key)}
          >
            <Icon name="chevron" size={11} className={open && folds ? "is-open" : undefined} />
          </button>
          {/* the whole row opens and closes the task like the arrow; a task not shown yet opens
              and shows its chat */}
          <button
            className="ws-row-pick"
            aria-expanded={folds ? open : undefined}
            onClick={() => {
              if (!shown) {
                onSelect(target);
                if (!open) toggle(key);
              } else if (folds) {
                toggle(key);
              }
            }}
            title={n.title || n.taskId}
          >
            <Dot state={taskState(n)} />
            <span className="ws-row-main">
              <span className="ws-row-title">
                <span className="ws-task-id">{n.taskId}</span> {n.title}
              </span>
              <span className="ws-row-meta faint">
                {n.sessions.length === 0 ? w.noSession : w.sessionCount(n.sessions.length)}
                {n.repo && ` · ${repoName(n.repo)}`}
              </span>
            </span>
          </button>
        </div>
        {folds && (
          <div className={`ws-task-kids${open ? " is-open" : ""}`} inert={!open}>
            <div>{n.sessions.map((s) => sessionRow(s, true))}</div>
          </div>
        )}
      </div>
    );
  };

  const outsideRow = (o: OutsideSession) => {
    const state = w.state[o.state] ?? o.state;
    return (
      <button
        key={o.id}
        className={`ws-row ws-row--session${isOn({ kind: "outside", id: o.id }) ? " is-on" : ""}`}
        onClick={() => onSelect({ kind: "outside", id: o.id })}
        title={`${o.title} · ${state} · ${o.openIn}`}
      >
        <Dot state={o.state} />
        <span className="ws-row-main">
          <span className="ws-row-title">{o.title}</span>
          <span className="ws-row-meta faint">
            {`${o.openIn} · ${repoName(o.cwd)}`}
            {o.updatedAt && ` · ${formatHourMinute(o.updatedAt)}`}
          </span>
        </span>
      </button>
    );
  };

  const outside = snapshot.outside ?? [];
  const empty = tree.projects.length === 0 && tree.free.length === 0 && outside.length === 0;
  return (
    <nav className="ws-tree" aria-label={w.title}>
      <div className="ws-tree-top">
        <button className="btn btn--sm ws-new" title={w.newSessionHint} onClick={onNew}>
          <span className="ws-plus" aria-hidden="true">
            +
          </span>
          {w.newSession}
        </button>
        <HideSideButton side="left" name={vi.side.workspace} onHide={onHide} />
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
        {outside.length > 0 && (
          <section className="ws-group">
            <button className="ws-group-head" aria-expanded={!closed.outside} title={w.outsideGroupHint}
              onClick={() => toggle("outside")}>
              <Icon name="chevron" size={11} className={!closed.outside ? "is-open" : undefined} />
              <span>{w.outsideGroup}</span>
            </button>
            {!closed.outside && outside.map(outsideRow)}
          </section>
        )}
      </div>
    </nav>
  );
}
