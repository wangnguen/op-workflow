import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import type { FlowAction, TaskDetail } from "../../ipc/types";
import { formatDateTime, formatUsd } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";

const t = vi.flow.talk;

interface Props {
  detail: TaskDetail;
  /** a button of the window is busy */
  disabled: boolean;
  /** resolves to whether the call worked */
  onAction: (a: FlowAction) => Promise<boolean>;
}

/** Under the strip of a failed step: talk the failure over with the AI (read only), then let the
 *  step go on the way agreed, or run it again from the start with the conversation. */
export function TalkPanel({ detail: d, disabled, onAction }: Props) {
  const [text, setText] = useState("");
  const turns = d.talk ?? [];
  const busy = d.talkBusy === true;
  const canGoOn = d.canGoOn === true;
  const list = useRef<HTMLOListElement>(null);
  // the newest message stays in view
  useEffect(() => {
    list.current?.scrollTo({ top: list.current.scrollHeight, behavior: "smooth" });
  }, [turns.length, busy]);

  const ask = () => {
    const q = text.trim();
    if (!q) return;
    void onAction({ type: "talk", text: q }).then((ok) => ok && setText(""));
  };
  const goOn = () => {
    void onAction({ type: "goOn", note: text.trim() }).then((ok) => ok && setText(""));
  };
  const locked = disabled || busy;

  return (
    <section className="talk" aria-label={t.title}>
      <header className="talk-head">
        <strong>{t.title}</strong>
        <small className="faint">{t.hint}</small>
      </header>
      {(turns.length > 0 || busy) && (
        <ol className="talk-turns" ref={list} aria-live="polite">
          {turns.map((turn, i) => (
            <li key={`${turn.at}-${i}`} className={`talk-turn ${turn.fromUser ? "talk-turn--user" : "talk-turn--ai"}`}>
              <span className="talk-who faint">
                {turn.fromUser ? t.you : t.ai} · {formatDateTime(turn.at)}
                {turn.costUsd !== undefined && ` · ${formatUsd(turn.costUsd)}`}
              </span>
              <p className="talk-text">{turn.text}</p>
            </li>
          ))}
          {busy && (
            <li className="talk-turn talk-turn--ai talk-turn--busy">
              <span className="talk-who faint">{t.ai}</span>
              <p className="talk-text faint">{t.answering}</p>
            </li>
          )}
        </ol>
      )}
      <textarea
        className="input talk-input"
        rows={2}
        value={text}
        placeholder={t.placeholder}
        disabled={disabled}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !locked) ask();
        }}
      />
      <div className="task-gate-choices talk-choices">
        <div className="task-gate-choice">
          {busy ? (
            <button className="btn btn--sm" disabled={disabled} onClick={() => void onAction({ type: "stop" })}>
              <Icon name="pause" size={12} />
              {t.stop}
            </button>
          ) : (
            <button className={canGoOn ? "btn btn--sm" : "btn btn--primary btn--sm"} disabled={locked || !text.trim()} onClick={ask}>
              {t.ask}
            </button>
          )}
          <small className="faint">{busy ? t.stopHint : t.askHint}</small>
        </div>
        <div className="task-gate-choice">
          <button className="btn btn--primary btn--sm" disabled={locked || !canGoOn} onClick={goOn}>
            <Icon name="play" size={12} />
            {t.goOn}
          </button>
          <small className="faint">{canGoOn ? t.goOnHint : t.goOnGone}</small>
        </div>
      </div>
    </section>
  );
}
