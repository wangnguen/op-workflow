import { useCallback, useEffect, useRef } from "react";
import { agentLabel, stepLabel, vi } from "../../i18n/vi";
import { POLL_MS, getStepChat } from "../../ipc/client";
import type { ChatItem, RunSummary, TaskDetail } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { usePolling } from "../../shared/hooks";
import { Markdown } from "../../shared/Markdown";
import { modelText } from "../../shared/models";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { chatRun, outputHead } from "./runs";

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
        {data?.items.map((item, i) => <Item key={i} item={item} />)}
        {live && !stale && <p className="faint chat-waiting">{t.waiting}</p>}
      </div>
    </div>
  );
}

function Item({ item }: { item: ChatItem }) {
  switch (item.kind) {
    case "prompt":
      return (
        <details className="chat-item chat-prompt">
          <summary>{t.prompt}</summary>
          <pre className="task-text">{item.text}</pre>
        </details>
      );
    case "text":
      return (
        <div className="chat-item chat-text">
          <Markdown text={item.text} className="task-md" />
        </div>
      );
    case "thinking":
      return (
        <details className="chat-item chat-thinking">
          <summary>{t.thinking}</summary>
          <p>{item.text}</p>
        </details>
      );
    case "tool":
      return <ToolItem item={item} />;
    case "end":
      return (
        <div className={`chat-end${item.failed ? " is-failed" : ""}`}>
          {(item.tool && t.end[item.tool]) ?? (item.tool ? t.endOther(item.tool) : t.end.success)}
          {item.text && <p>{item.text}</p>}
        </div>
      );
    case "error":
      return <p className="note note--error chat-item">{item.text}</p>;
  }
}

function ToolItem({ item }: { item: ChatItem }) {
  const peek = item.output ? outputHead(item, 1).head : "";
  return (
    <div className={`chat-tool${item.failed ? " is-failed" : ""}${item.denied ? " is-denied" : ""}`}>
      <div className="chat-tool-head">
        <span className="chat-tool-name">{item.tool}</span>
        {item.text && <span className="mono chat-tool-input">{item.text}</span>}
        {item.denied && <span className="chip chip--warn">{t.denied}</span>}
        {item.failed && !item.denied && <span className="chip chip--failed">{t.failed}</span>}
      </div>
      {item.denied && <p className="chat-tool-why">{item.denied}</p>}
      {item.output !== undefined && item.output !== "" && (
        <details className="chat-tool-out">
          <summary>
            {t.output}
            {peek && <span className="mono faint chat-tool-peek">{peek}</span>}
          </summary>
          <pre className="chat-tool-pre">
            {item.output}
            {item.outputCut ? `\n${t.more(item.outputCut)}` : ""}
          </pre>
        </details>
      )}
    </div>
  );
}
