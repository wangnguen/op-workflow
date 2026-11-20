import { useCallback, useEffect, useRef } from "react";
import { agentLabel, stepLabel, vi } from "../../i18n/vi";
import { POLL_MS, getStepChat } from "../../ipc/client";
import type { RunSummary, TaskDetail } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { usePolling } from "../../shared/hooks";
import { modelText } from "../../shared/models";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { ChatItemView } from "../chat/ChatItems";
import { chatRun } from "./runs";

const t = vi.flow.chat;
const tp = vi.flow.progress;

interface Props {
  detail: TaskDetail;
  /** run picked in Tiến trình or in the list here; undefined = the running or latest one */
  runId?: number;
  busy?: string;
  onPick: (runId: number) => void;
  onResume: (runId: number) => void;
}

/** Chat: the transcript of one run (the prompt, what the AI wrote, each tool call with a short
 *  result). Follows the run live while it runs. */
export function ChatTab({ detail: d, runId, busy, onPick, onResume }: Props) {
  const run = chatRun(d.runs, runId);
  if (!run) return <p className="dim">{t.empty}</p>;
  return <ChatBody detail={d} run={run} busy={busy} onPick={onPick} onResume={onResume} />;
}

function runOption(r: RunSummary): string {
  const state = r.state === "running" ? ` (${tp.state.running})` : r.state === "failed" ? ` (${tp.state.failed})` : "";
  const round = r.round ? ` · ${vi.orbit.round(r.round)}` : "";
  return `${formatHourMinute(r.startedAt)} · ${stepLabel[r.step]}${round}${state}`;
}

function ChatBody({ detail: d, run, busy, onPick, onResume }: { detail: TaskDetail; run: RunSummary } & Omit<Props, "detail" | "runId">) {
  const live = run.state === "running";
  const fetcher = useCallback(() => getStepChat(d.wpId, run.id), [d.wpId, run.id]);
  // live: as often as the task window; a finished run only changes when its log is deleted
  const polled = usePolling(fetcher, live ? POLL_MS : 60_000);
  const data = polled.data;
  // while another run loads, the old transcript stays (faded) instead of a blank pane
  const stale = data !== undefined && data.run.id !== run.id;
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const count = data?.items.length ?? 0;
  useEffect(() => {
    const el = list.current;
    if (el && stick.current && live) el.scrollTop = el.scrollHeight;
  }, [count, live]);
  const model = modelText(run.model, run.effort);
  const taskRunning = d.status === "running";
  return (
    <div className="chat">
      <div className="chat-head">
        <label className="chat-run">
          <span className="faint">{t.runLabel}</span>
          <select className="input" value={run.id} onChange={(e) => onPick(Number(e.target.value))}>
            {d.runs.map((r) => (
              <option key={r.id} value={r.id}>
                {runOption(r)}
              </option>
            ))}
          </select>
        </label>
        <span className={`chip chip--${run.agent}`}>{agentLabel[run.agent]}</span>
        {model && <span className="faint">{model}</span>}
        {live && (
          <span className="chat-live">
            <StatusIcon state="running" />
            {t.live}
          </span>
        )}
        <span className="chat-head-gap" />
        {run.resume && (
          <button
            className="btn btn--sm"
            disabled={busy !== undefined || taskRunning}
            title={taskRunning ? tp.resumeBusy : tp.resumeHint(run.resume)}
            onClick={() => onResume(run.id)}
          >
            <Icon name="laptop" size={12} />
            {tp.resume}
          </button>
        )}
      </div>
      {polled.error && <p className="note note--error">{polled.error}</p>}
      <div
        className={`chat-list${stale ? " is-stale" : ""}`}
        ref={list}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 32;
        }}
      >
        {!data && !polled.error && <p className="dim">{t.loading}</p>}
        {data?.missing && <p className="dim">{t.missing}</p>}
        {data && data.dropped > 0 && <p className="faint">{t.dropped(data.dropped)}</p>}
        {data?.items.map((item, i) => <ChatItemView key={i} item={item} />)}
        {live && !stale && <p className="faint chat-waiting">{t.waiting}</p>}
      </div>
    </div>
  );
}
