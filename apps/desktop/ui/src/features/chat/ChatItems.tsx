import { vi } from "../../i18n/vi";
import type { ChatItem } from "../../ipc/types";
import { Markdown } from "../../shared/Markdown";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { outputHead } from "../task/runs";

const t = vi.flow.chat;

/** One item of a chat: what the app or the user sent, what the AI wrote, a tool call (its result
 *  folded), a marker (stopped, compacted, the run's end), an error or a line from the app (not
 *  sent to the AI). Shared by the task window's Chat tab and the Workspace chat. */
export function ChatItemView({ item }: { item: ChatItem }) {
  switch (item.kind) {
    case "prompt":
      return (
        <details className="chat-item chat-prompt">
          <summary>{t.prompt}</summary>
          <pre className="task-text">{item.text}</pre>
        </details>
      );
    case "user":
      return (
        <div className="chat-item chat-user">
          <p>{item.text}</p>
        </div>
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
      if (item.tool === "interrupted" || item.tool === "compacted") {
        return <div className="chat-mark">{t.mark[item.tool]}</div>;
      }
      return (
        <div className={`chat-end${item.failed ? " is-failed" : ""}`}>
          {(item.tool && t.end[item.tool]) ?? (item.tool ? t.endOther(item.tool) : t.end.success)}
          {item.text && <p>{item.text}</p>}
        </div>
      );
    case "error":
      return <p className="note note--error chat-item">{item.text}</p>;
    case "note":
      return (
        <p className="chat-note">
          <Icon name="info" size={12} />
          <span>{item.text}</span>
        </p>
      );
  }
}

function ToolItem({ item }: { item: ChatItem }) {
  const peek = item.output ? outputHead(item, 1).head : "";
  const children = item.children ?? [];
  return (
    <div className={`chat-tool${item.failed ? " is-failed" : ""}${item.denied ? " is-denied" : ""}`} data-tool-id={item.toolId}>
      <div className="chat-tool-head">
        <span className="chat-tool-name">{item.tool}</span>
        {item.text && <span className="mono chat-tool-input">{item.text}</span>}
        {item.running && (
          <span className="chip chat-tool-running">
            <StatusIcon state="running" size={11} />
            {t.subRunning}
          </span>
        )}
        {item.denied && <span className="chip chip--warn">{t.denied}</span>}
        {item.failed && !item.denied && <span className="chip chip--failed">{t.failed}</span>}
      </div>
      {item.denied && <p className="chat-tool-why">{item.denied}</p>}
      {(children.length > 0 || item.running) && <SubagentSteps items={children} running={item.running === true} />}
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

/** One line for what a subagent did last (its last tool call or message). */
function lastStep(items: ChatItem[]): string {
  const last = items[items.length - 1];
  if (!last) return "";
  const first = last.text.split("\n").find((l) => l.trim() !== "")?.trim() ?? "";
  return last.kind === "tool" ? `${last.tool ?? ""} ${first}`.trim() : first;
}

/** What a subagent did under its Agent / Task call, folded like the extension shows it: the count
 *  and the last step in the summary (it follows the subagent while it runs), every step inside. */
function SubagentSteps({ items, running }: { items: ChatItem[]; running: boolean }) {
  const last = lastStep(items);
  return (
    <details className={`chat-sub${running ? " is-running" : ""}`}>
      <summary>
        {running ? t.subStepsRunning(items.length) : t.subSteps(items.length)}
        {last && <span className="mono faint chat-sub-last">{last}</span>}
      </summary>
      <div className="chat-sub-list">
        {items.map((child, i) => (
          <ChatItemView key={i} item={child} />
        ))}
      </div>
    </details>
  );
}
