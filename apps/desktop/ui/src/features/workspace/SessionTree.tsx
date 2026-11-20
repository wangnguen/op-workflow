import { type ReactNode, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, restoreSession } from "../../ipc/client";
import type { ArchivedSession, OutsideSession, SessionState, SessionView, WorkspaceSnapshot } from "../../ipc/types";
import { formatDateTime, formatHourMinute } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { HideSideButton } from "../../shared/ui/SidePanel";
import {
  OTHER_PROJECT,
  type RepoGroup,
  type Selection,
  type TaskNode,
  buildTree,
  byRepo,
  repoName,
  samePick,
  taskState,
  urgentState,
} from "./chatState";

const w = vi.workspace;

interface Props {
  snapshot: WorkspaceSnapshot;
  selected: Selection | undefined;
  onSelect: (s: Selection) => void;
  onNew: () => void;
  /** hide the panel the list sits in */
  onHide: () => void;
  /** the list changed here (a session restored): read Workspace again */
  onChanged: () => void;
}

function Dot({ state }: { state: SessionState | undefined }) {
  return <span className={`ws-dot ws-dot--${state ?? "none"}`} aria-hidden="true" />;
}

/** Rows that slide open and closed under their head. */
function Fold({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div className={`ws-fold${open ? " is-open" : ""}`} inert={!open}>
      <div>{children}</div>
    </div>
  );
}

function stateLabel(s: SessionView): string {
  return s.state === "outside" && s.outside ? w.outsideAt(s.outside) : (w.state[s.state] ?? s.state);
}

/** Project → task → sessions; then the sessions without a task and those open outside the app,
 *  each under its repo; then "Đã lưu trữ" (folded). Every session row opens its chat. */
export function SessionTree({ snapshot, selected, onSelect, onNew, onHide, onChanged }: Props) {
  const tree = buildTree(snapshot);
  // open or closed by hand; a head not touched yet uses its default (open, "Đã lưu trữ" closed)
  const [folds, setFolds] = useState<Record<string, boolean>>({});
  const isOpen = (key: string, byDefault = true) => folds[key] ?? byDefault;
  const toggle = (key: string, byDefault = true) => setFolds((f) => ({ ...f, [key]: !(f[key] ?? byDefault) }));
  const isOn = (s: Selection) => samePick(selected, s);
  const [restoring, setRestoring] = useState<string>();
  const [restoreError, setRestoreError] = useState<string>();

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
          {` · ${formatHourMinute(s.updatedAt)}`}
        </span>
      </span>
    </button>
  );

  const taskRow = (n: TaskNode) => {
    const key = `task-${n.wpId}`;
    const open = isOpen(key);
    const target: Selection = n.sessions[0] ? { kind: "session", id: n.sessions[0].id } : { kind: "task", wpId: n.wpId };
    // the task's chat is shown (the task itself or one of its sessions)
    const shown =
      isOn({ kind: "task", wpId: n.wpId }) || n.sessions.some((s) => isOn({ kind: "session", id: s.id }));
    // folded with its shown session inside: the task row carries the mark
    const on = isOn({ kind: "task", wpId: n.wpId }) || (shown && !open);
    const hasKids = n.sessions.length > 0;
    return (
      <div key={key} className="ws-task">
        <div className={`ws-row ws-row--task${on ? " is-on" : ""}`}>
          <button
            className="ws-twist"
            aria-label={open ? w.collapse : w.expand}
            aria-expanded={open}
            disabled={!hasKids}
            onClick={() => toggle(key)}
          >
            <Icon name="chevron" size={11} className={open && hasKids ? "is-open" : undefined} />
          </button>
          {/* the whole row opens and closes the task like the arrow; a task not shown yet opens
              and shows its chat */}
          <button
            className="ws-row-pick"
            aria-expanded={hasKids ? open : undefined}
            onClick={() => {
              if (!shown) {
                onSelect(target);
                if (!open) toggle(key);
              } else if (hasKids) {
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
        {hasKids && <Fold open={open}>{n.sessions.map((s) => sessionRow(s, true))}</Fold>}
      </div>
    );
  };

  /** A repo and its sessions under it: the row opens and closes them. Folded, the repo row
   *  shows the most urgent state and carries the mark of the session shown inside. */
  const repoRow = <T,>(
    group: string,
    g: RepoGroup<T>,
    stateOf: (item: T) => SessionState,
    shown: (item: T) => boolean,
    row: (item: T) => ReactNode,
  ) => {
    const key = `${group}:${g.key}`;
    const open = isOpen(key);
    const on = !open && g.items.some(shown);
    return (
      <div key={key} className="ws-task">
        <div className={`ws-row ws-row--task${on ? " is-on" : ""}`}>
          <button className="ws-twist" aria-label={open ? w.collapse : w.expand} aria-expanded={open} onClick={() => toggle(key)}>
            <Icon name="chevron" size={11} className={open ? "is-open" : undefined} />
          </button>
          <button className="ws-row-pick" aria-expanded={open} onClick={() => toggle(key)} title={g.repo}>
            <Dot state={urgentState(g.items.map(stateOf))} />
            <span className="ws-row-main">
              <span className="ws-row-title ws-repo-name">{g.name}</span>
              <span className="ws-row-meta faint">{w.sessionCount(g.items.length)}</span>
            </span>
          </button>
        </div>
        <Fold open={open}>{g.items.map(row)}</Fold>
      </div>
    );
  };

  const outsideRow = (o: OutsideSession) => {
    const state = w.state[o.state] ?? o.state;
    return (
      <button
        key={o.id}
        className={`ws-row ws-row--session is-nested${isOn({ kind: "outside", id: o.id }) ? " is-on" : ""}`}
        onClick={() => onSelect({ kind: "outside", id: o.id })}
        title={`${o.title} · ${state} · ${o.openIn}`}
      >
        <Dot state={o.state} />
        <span className="ws-row-main">
          <span className="ws-row-title">{o.title}</span>
          <span className="ws-row-meta faint">
            {o.openIn}
            {o.updatedAt && ` · ${formatHourMinute(o.updatedAt)}`}
          </span>
        </span>
      </button>
    );
  };

  const restore = async (id: string) => {
    setRestoring(id);
    setRestoreError(undefined);
    try {
      await restoreSession(id);
      onChanged();
    } catch (e) {
      setRestoreError(errorMessage(e));
    } finally {
      setRestoring(undefined);
    }
  };

  const archivedRow = (a: ArchivedSession) => (
    <div key={a.id} className="ws-row ws-row--archived">
      <span className="ws-row-main">
        <span className="ws-row-title">{a.title}</span>
        <span className="ws-row-meta faint">
          {[a.outside ? w.archived.outside : a.taskId, repoName(a.repo), w.archived.at(formatDateTime(a.archivedAt))]
            .filter(Boolean)
            .join(" · ")}
        </span>
      </span>
      <button
        className="btn btn--sm ws-restore"
        title={w.archived.restoreHint}
        disabled={restoring !== undefined}
        onClick={() => void restore(a.id)}
      >
        {restoring === a.id ? w.archived.restoring : w.archived.restore}
      </button>
    </div>
  );

  /** A group's head (Task đang làm, Phiên riêng, …) and what it folds. */
  const group = (key: string, title: string, body: ReactNode, opts: { hint?: string; byDefault?: boolean } = {}) => {
    const open = isOpen(key, opts.byDefault);
    return (
      <section key={key} className="ws-group">
        <button className="ws-group-head" aria-expanded={open} title={opts.hint} onClick={() => toggle(key, opts.byDefault)}>
          <Icon name="chevron" size={11} className={open ? "is-open" : undefined} />
          <span>{title}</span>
        </button>
        <Fold open={open}>{body}</Fold>
      </section>
    );
  };

  const outside = snapshot.outside ?? [];
  const archived = snapshot.archived ?? [];
  const sessionShown = (s: SessionView) => isOn({ kind: "session", id: s.id });
  const outsideShown = (o: OutsideSession) => isOn({ kind: "outside", id: o.id });
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
        {tree.projects.map((p) =>
          group(`project-${p.project}`, p.project === OTHER_PROJECT ? w.tasks : p.project, p.tasks.map(taskRow)),
        )}
        {tree.free.length > 0 &&
          group(
            "free",
            w.free,
            byRepo(tree.free, (s) => s.repo).map((g) =>
              repoRow("free", g, (s) => s.state, sessionShown, (s) => sessionRow(s, true)),
            ),
          )}
        {outside.length > 0 &&
          group(
            "outside",
            w.outsideGroup,
            byRepo(outside, (o) => o.cwd).map((g) => repoRow("outside", g, (o) => o.state, outsideShown, outsideRow)),
            { hint: w.outsideGroupHint },
          )}
        {archived.length > 0 &&
          group(
            "archived",
            w.archived.group(archived.length),
            <>
              {restoreError && <p className="note note--error ws-tree-note">{restoreError}</p>}
              {archived.map(archivedRow)}
            </>,
            { hint: w.archived.groupHint, byDefault: false },
          )}
      </div>
    </nav>
  );
}
