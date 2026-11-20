import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorCode, errorMessage, getOutsideChat } from "../../ipc/client";
import type { OutsideChat, OutsideSession } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { ChatItemView } from "../chat/ChatItems";
import { repoName } from "./chatState";

const t = vi.workspace.outsidePane;
/** The transcript is read again this often while the pane is shown (only a changed file is
 *  parsed again). */
const POLL_MS = 3000;

interface Props {
  id: string;
  /** what the list knows about it, shown while the conversation is read */
  known?: OutsideSession;
}

/** Cheap fingerprint: a read that changed nothing does not draw the chat again. */
const mark = (c: OutsideChat) => {
  const last = c.items[c.items.length - 1];
  return [c.items.length, c.dropped, c.session.state, c.session.title, last?.text.length, last?.output?.length].join("|");
};

/** A Claude session opened outside the app (VS Code, a terminal): its conversation, read-only. */
export function OutsidePane({ id, known }: Props) {
  const [chat, setChat] = useState<OutsideChat>();
  const [closed, setClosed] = useState(false);
  const [error, setError] = useState<string>();
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const seen = useRef("");

  useEffect(() => {
    setChat(undefined);
    setClosed(false);
    setError(undefined);
    stick.current = true;
    seen.current = "";
    let alive = true;
    const read = async () => {
      try {
        const c = await getOutsideChat(id);
        if (!alive) return;
        setClosed(false);
        setError(undefined);
        const m = mark(c);
        if (m !== seen.current) {
          seen.current = m;
          setChat(c);
        }
      } catch (e) {
        if (!alive) return;
        if (errorCode(e) === "session.closed") setClosed(true);
        else setError(errorMessage(e));
      }
    };
    void read();
    const timer = window.setInterval(() => void read(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [id]);

  useEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [chat]);

  const s = chat?.session ?? known;
  const where = s?.openIn ?? "";
  return (
    <div className="ws-chat">
      <div className="ws-chat-body">
        <header className="ws-chat-head">
          <div className="ws-chat-col ws-chat-head-in">
            <div className="ws-chat-titles">
              <h2 title={s?.title}>{s?.title ?? t.title}</h2>
              <p className="faint">
                <span className="chip">{t.chip}</span>
                {s && (
                  <span className="mono" title={s.cwd}>
                    {repoName(s.cwd)}
                  </span>
                )}
                {s && !closed && <span>{vi.workspace.state[s.state] ?? s.state}</span>}
                {where && !closed && <span>{t.openIn(where)}</span>}
                {s?.updatedAt && <span>{t.updated(formatHourMinute(s.updatedAt))}</span>}
              </p>
            </div>
          </div>
        </header>
        <div
          className="ws-chat-list"
          ref={list}
          onScroll={(e) => {
            const el = e.currentTarget;
            stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
          }}
        >
          <div className="ws-chat-col">
            {error && <p className="note note--error">{error}</p>}
            {!chat && !error && !closed && <p className="dim">{vi.workspace.chat.loading}</p>}
            {chat && chat.dropped > 0 && <p className="faint">{vi.workspace.chat.dropped(chat.dropped)}</p>}
            {chat && chat.items.length === 0 && <p className="ws-chat-empty dim">{t.empty}</p>}
            {chat?.items.map((item, i) => <ChatItemView key={i} item={item} />)}
          </div>
        </div>
        <footer className="ws-composer">
          <div className="ws-chat-col">
            <p className="note">{closed ? t.closed : t.readOnly(where || t.elsewhere)}</p>
          </div>
        </footer>
      </div>
    </div>
  );
}
