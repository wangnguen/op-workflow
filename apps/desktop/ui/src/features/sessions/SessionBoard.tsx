import { useState } from "react";
import { agentLabel, vi } from "../../i18n/vi";
import { archiveOutside, hideSession } from "../../ipc/client";
import type { AgentKind, OrbitSnapshot, WorkspaceSnapshot } from "../../ipc/types";
import { formatRelative, formatUsd } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import type { Go } from "../orbit/primary";
import { sessionTable, tally, type SessionRow } from "./sessions";

interface Props {
  snapshot: OrbitSnapshot;
  /** Workspace sessions (the app's and those open outside it) */
  workspace?: WorkspaceSnapshot;
  now: number;
  /** "Tới phiên" and the row's title */
  onGo?: (go: Go) => void;
  /** open the task window (the task id on a row) */
  onOpen?: (taskId: string) => void;
  /** a session was archived: read everything again */
  onChanged?: () => void;
  onError?: (e: unknown) => void;
}

const AVATAR: Record<AgentKind, string> = { claude: "CL", codex: "CX", shell: "SH" };
const t = vi.sessions;

function Row({ r, now, onGo, onOpen, onArchive, archiving }: {
  r: SessionRow;
  now: number;
  onGo?: (go: Go) => void;
  onOpen?: (taskId: string) => void;
  onArchive: (r: SessionRow) => void;
  archiving: boolean;
}) {
  const goHint = r.kind === "outside" ? t.goOutsideHint : r.asks ? t.goAskHint : t.goHint;
  const where = r.kind === "outside" ? (r.openIn ? `${t.outside} · ${r.openIn}` : t.outside) : r.kind === "task" ? t.noSession : undefined;
  const ctx = r.contextPct;
  return (
    <tr className={`s-row s-row--${r.status}`}>
      <td className="s-cell-session">
        <span className="s-session">
          {r.agent ? (
            <span className={`s-avatar s-avatar--${r.agent}`} title={agentLabel[r.agent]}>
              {AVATAR[r.agent]}
            </span>
          ) : (
            <span className="s-avatar s-avatar--task" aria-hidden="true">
              <Icon name="branch" size={13} />
            </span>
          )}
          <span className="s-titles">
            {onGo ? (
              <button type="button" className="s-title-btn" title={`${r.title}\n${goHint}`} onClick={() => onGo(r.to)}>
                {r.title}
              </button>
            ) : (
              <b title={r.title}>{r.title}</b>
            )}
            {(r.taskId || where) && (
              <span className="s-sub faint">
                {r.taskId &&
                  (onOpen ? (
                    <button type="button" className="link-btn mono" title={vi.flow.openHint} onClick={() => onOpen(r.taskId ?? "")}>
                      {r.taskId}
                    </button>
                  ) : (
                    <span className="mono">{r.taskId}</span>
                  ))}
                {where && <span>{where}</span>}
              </span>
            )}
          </span>
        </span>
      </td>
      <td>
        <span className="s-state">
          <span className={`s-dot s-dot--${r.status}`} aria-hidden="true" />
          {r.stateText}
        </span>
      </td>
      <td className="s-cell-activity">
        <span className="s-activity" title={r.activity}>
          {r.activity || <span className="faint">—</span>}
        </span>
      </td>
      <td className="s-cell-num faint" title={r.updatedAt}>
        {formatRelative(r.updatedAt, now)}
      </td>
      <td className="s-cell-ctx">
        {ctx !== undefined ? (
          <span className="s-ctx" title={t.contextHint}>
            <span className={`bar${r.agent === "codex" ? " bar--codex" : ""}${ctx >= 90 ? " bar--hot" : ""}`}>
              <span style={{ width: `${Math.min(100, ctx)}%` }} />
            </span>
            <span className="faint">{Math.round(ctx)}%</span>
          </span>
        ) : (
          <span className="faint">—</span>
        )}
      </td>
      <td className="s-cell-num">{r.costUsd !== undefined ? formatUsd(r.costUsd) : <span className="faint">—</span>}</td>
      <td className="s-cell-actions">
        {onGo && (
          <button className={`btn btn--sm${r.asks ? " btn--primary" : ""}`} title={goHint} onClick={() => onGo(r.to)}>
            <Icon name="chat" size={12} />
            {r.asks ? t.goAsk : t.go}
          </button>
        )}
        {r.kind !== "task" && (
          <button
            className="btn btn--sm btn--ghost"
            title={r.archiveBlocked ?? (r.kind === "outside" ? t.archiveOutsideHint : t.archiveHint)}
            disabled={archiving || r.archiveBlocked !== undefined}
            onClick={() => onArchive(r)}
          >
            <Icon name="archive" size={12} />
            {t.archive}
          </button>
        )}
      </td>
    </tr>
  );
}

/** "Bảng": every session in one table, grouped by repo. */
export function SessionBoard({ snapshot, workspace, now, onGo, onOpen, onChanged, onError }: Props) {
  const groups = sessionTable(snapshot, workspace);
  const [archiving, setArchiving] = useState<string>();
  const archive = async (r: SessionRow) => {
    setArchiving(r.id);
    try {
      const id = r.id.slice(r.id.indexOf(":") + 1);
      await (r.kind === "outside" ? archiveOutside(id) : hideSession(id));
      onChanged?.();
    } catch (e) {
      onError?.(e);
    } finally {
      setArchiving(undefined);
    }
  };
  if (groups.length === 0) {
    return (
      <div className="session-table-wrap">
        <p className="s-table-empty dim">{t.empty}</p>
      </div>
    );
  }
  return (
    <div className="session-table-wrap">
      <table className="s-table">
        <thead>
          <tr>
            <th scope="col">{t.colSession}</th>
            <th scope="col">{t.colState}</th>
            <th scope="col">{t.colActivity}</th>
            <th scope="col" className="s-cell-num">{t.colUpdated}</th>
            <th scope="col">{t.colContext}</th>
            <th scope="col" className="s-cell-num">{t.colCost}</th>
            <th scope="col">
              <span className="sr-only">{t.colActions}</span>
            </th>
          </tr>
        </thead>
        {groups.map((g) => {
          const n = tally(g.rows);
          return (
            <tbody key={g.key}>
              <tr className="s-group-row">
                <th colSpan={7} scope="rowgroup">
                  <span className="s-group-head" title={g.path}>
                    <Icon name="folder" size={13} />
                    <span className="s-group-name">{g.name || t.noRepo}</span>
                    <span className="faint">{t.groupTally(g.rows.length, n.waiting, n.running)}</span>
                  </span>
                </th>
              </tr>
              {g.rows.map((r) => (
                <Row key={r.id} r={r} now={now} onGo={onGo} onOpen={onOpen} onArchive={(x) => void archive(x)}
                  archiving={archiving !== undefined} />
              ))}
            </tbody>
          );
        })}
      </table>
    </div>
  );
}
