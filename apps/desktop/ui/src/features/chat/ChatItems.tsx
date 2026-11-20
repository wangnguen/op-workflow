import { vi } from "../../i18n/vi";
import type { ChatItem } from "../../ipc/types";
import { Markdown } from "../../shared/Markdown";
import { outputHead } from "../task/runs";

const t = vi.flow.chat;

/** One item of a chat: what the app or the user sent, what the AI wrote, a tool call (its result
 *  folded), a marker (stopped, compacted, the run's end) or an error. Shared by the task window's
 *  Chat tab and the Workspace chat. */
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
