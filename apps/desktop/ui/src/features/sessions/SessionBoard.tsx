import { agentLabel, vi } from "../../i18n/vi";
import type { AgentKind, OrbitSnapshot, SessionStatus } from "../../ipc/types";
import { groupSessions } from "./sessions";

interface Props {
  snapshot: OrbitSnapshot;
}

const AVATAR: Record<AgentKind, string> = { claude: "CL", codex: "CX", shell: "SH" };

const COLUMNS: { key: SessionStatus; title: string }[] = [
  { key: "waiting", title: vi.sessions.needsYou },
  { key: "running", title: vi.sessions.running },
  { key: "idle", title: vi.sessions.idle },
];

/** Session cards grouped Cần bạn / Đang chạy / Rảnh. */
export function SessionBoard({ snapshot }: Props) {
  const groups = groupSessions(snapshot);
  return (
    <div className="session-board">
      {COLUMNS.map((col) => (
        <section key={col.key} className={`s-column s-column--${col.key}`}>
          <h2>
            {col.title} <span className="faint">{groups[col.key].length}</span>
          </h2>
          {groups[col.key].length === 0 && <div className="column-empty">{vi.sessions.none}</div>}
          {groups[col.key].map((s) => (
            <article key={s.id} className={`s-card s-card--${s.status} s-card--${s.agent}`}>
              <header>
                <span className={`s-avatar s-avatar--${s.agent}`}>{AVATAR[s.agent]}</span>
                <span className="s-titles">
                  <b>{s.title}</b>
                  <span className="faint">
                    {agentLabel[s.agent]} · {s.detail}
                  </span>
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
              <button className="btn btn--sm" disabled title={vi.sessions.openPaneLater}>
                {vi.sessions.openPane}
              </button>
            </article>
          ))}
        </section>
      ))}
    </div>
  );
}
