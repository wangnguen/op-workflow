import { agentLabel, vi } from "../../i18n/vi";
import type { AgentKind, OrbitSnapshot, SessionStatus } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { StatusChip } from "../../shared/ui/StatusChip";
import { StartOver } from "../task/StartOver";
import { groupSessions, type SessionItem } from "./sessions";

interface Props {
  snapshot: OrbitSnapshot;
  /** centre the task on the map */
  onShow?: (taskId: string) => void;
  /** "Lấy issue" for a task in "Đang làm" */
  onIssues?: (taskId: string) => void;
  /** show the task's brief.md (absent in the browser preview) */
  onBrief?: (taskId: string) => void;
  /** tasks whose issues are being read right now */
  reading?: ReadonlySet<string>;
  /** open the task window (brief, plan, findings, push, log) */
  onOpen?: (taskId: string) => void;
  /** the OpenProject id of a task, for Làm lại từ đầu */
  wpIdOf?: (taskId: string) => number | undefined;
  /** a task was started over */
  onChanged?: () => void;
}

const AVATAR: Record<AgentKind, string> = { claude: "CL", codex: "CX", shell: "SH" };

const COLUMNS: { key: SessionStatus; title: string; hint: string }[] = [
  { key: "waiting", title: vi.sessions.needsYou, hint: vi.sessions.needsYouHint },
  { key: "running", title: vi.sessions.running, hint: vi.sessions.runningHint },
  { key: "idle", title: vi.sessions.idle, hint: vi.sessions.idleHint },
];

function Card({ s, onShow, onIssues, onBrief, onOpen, reading, wpIdOf, onChanged }: { s: SessionItem } & Omit<Props, "snapshot">) {
  const taskId = s.taskId;
  const isTask = !s.agent;
  const wpId = taskId && s.inApp ? wpIdOf?.(taskId) : undefined;
  return (
    <article className={`s-card s-card--${s.status}${s.agent ? ` s-card--${s.agent}` : " s-card--task"}`}>
      <header>
        {s.agent ? (
          <span className={`s-avatar s-avatar--${s.agent}`}>{AVATAR[s.agent]}</span>
        ) : (
          <span className="s-avatar s-avatar--task" aria-hidden="true">
            <Icon name="branch" size={14} />
          </span>
        )}
        <span className="s-titles">
          <b title={s.title}>{s.title}</b>
          <span className="faint" title={s.agent ? `${agentLabel[s.agent]} · ${s.detail}` : s.detail}>
            {s.agent ? `${agentLabel[s.agent]} · ${s.detail}` : s.detail}
          </span>
          {(s.nxStatus || s.repo || s.branch) && (
            <span className="s-meta">
              {s.nxStatus && <StatusChip name={s.nxStatus} color={s.nxStatusColor} />}
              {s.repo && (
                <span className="s-repo faint" title={s.repo}>
                  {s.repo}
                </span>
              )}
              {s.branch && (
                <span className="s-branch mono" title={s.branch}>
                  {s.branch}
                </span>
              )}
            </span>
          )}
        </span>
      </header>
      {s.contextPct !== undefined && (
        <div className="s-ctx">
          <div className={`bar${s.agent === "codex" ? " bar--codex" : ""}${s.contextPct >= 90 ? " bar--hot" : ""}`}>
            <span style={{ width: `${Math.min(100, s.contextPct)}%` }} />
          </div>
          <span className="faint">context {Math.round(s.contextPct)}%</span>
        </div>
      )}
      <footer className="s-actions">
        {taskId && onShow && (
          <button className="btn btn--sm btn--ghost" title={vi.sessions.showOnMapHint} onClick={() => onShow(taskId)}>
            <Icon name="dot" size={12} />
            {vi.sessions.showOnMap}
          </button>
        )}
        {isTask && taskId && onIssues && (
          <button
            className="btn btn--sm btn--ghost"
            title={vi.board.issuesHint}
            disabled={reading?.has(taskId)}
            onClick={() => onIssues(taskId)}
          >
            <Icon name="refresh" size={12} className={reading?.has(taskId) ? "is-spinning" : undefined} />
            {vi.board.issues}
          </button>
        )}
        {isTask && taskId && onBrief && (
          <button className="btn btn--sm" title={vi.board.briefHint} onClick={() => onBrief(taskId)}>
            <Icon name="file" size={12} />
            {vi.board.brief}
          </button>
        )}
        {taskId && s.inApp && onOpen && (
          <button className="btn btn--sm" title={vi.flow.openHint} onClick={() => onOpen(taskId)}>
            {vi.flow.open}
          </button>
        )}
        {wpId !== undefined && (
          <StartOver wpId={wpId} small disabled={s.status === "running"} onDone={() => onChanged?.()} />
        )}
        {!s.inApp && (
          <button className="btn btn--sm" disabled title={vi.sessions.openPaneLater}>
            {vi.sessions.openPane}
          </button>
        )}
      </footer>
    </article>
  );
}

/** Session cards grouped Cần bạn / Đang chạy / Rảnh. */
export function SessionBoard({ snapshot, ...actions }: Props) {
  const groups = groupSessions(snapshot);
  return (
    <div className="session-board">
      {COLUMNS.map((col) => (
        <section key={col.key} className={`s-column s-column--${col.key}`}>
          <header className="s-column-head">
            <h2>
              {col.title} <span className="column-count">{groups[col.key].length}</span>
            </h2>
            <span className="faint">{col.hint}</span>
          </header>
          {groups[col.key].length === 0 && <div className="column-empty">{vi.sessions.none}</div>}
          <div className="s-cards">
            {groups[col.key].map((s) => (
              <Card key={s.id} s={s} {...actions} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
