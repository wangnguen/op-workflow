import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { adoptOutside, archiveOutside, errorCode, errorMessage, forkOutside, getOutsideChat } from "../../ipc/client";
import type { OutsideChat, OutsideSession } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { ChatItemView } from "../chat/ChatItems";
import { Choice } from "./AskCard";
import { repoName } from "./chatState";

const t = vi.workspace.outsidePane;
/** The transcript is read again this often while the pane is shown (only a changed file is
 *  parsed again). */
const POLL_MS = 3000;

interface Props {
  id: string;
  /** what the list knows about it, shown while the conversation is read */
  known?: OutsideSession;
  /** a session of the app to show now (this one taken into the app, or its copy) */
  onOpen: (id: string) => void;
  /** taken off the list */
  onArchived: () => void;
}

/** Cheap fingerprint: a read that changed nothing does not draw the chat again. */
const mark = (c: OutsideChat) => {
  const last = c.items[c.items.length - 1];
  return [c.items.length, c.dropped, c.session.state, c.session.title, last?.text.length, last?.output?.length].join("|");
};

/** A Claude session opened outside the app (VS Code, a terminal): its conversation, read-only,
 *  and the ways to go on with it here. */
export function OutsidePane({ id, known, onOpen, onArchived }: Props) {
  const [chat, setChat] = useState<OutsideChat>();
  const [closed, setClosed] = useState(false);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState<"adopt" | "fork" | "archive">();
  const [failure, setFailure] = useState<string>();
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const seen = useRef("");

  useEffect(() => {
    setChat(undefined);
    setClosed(false);
    setError(undefined);
    setFailure(undefined);
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

  const act = async (what: "adopt" | "fork" | "archive") => {
    setBusy(what);
    setFailure(undefined);
    try {
      if (what === "archive") {
        await archiveOutside(id);
        onArchived();
      } else {
        const s = await (what === "adopt" ? adoptOutside(id) : forkOutside(id));
        onOpen(s.id);
      }
    } catch (e) {
      if (errorCode(e) === "session.closed") setClosed(true);
      else setFailure(errorMessage(e));
    } finally {
      setBusy(undefined);
    }
  };

  const s = chat?.session ?? known;
  const where = s?.openIn || t.elsewhere;
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
                {s?.openIn && !closed && <span>{t.openIn(s.openIn)}</span>}
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
            {failure && <p className="note note--error">{failure}</p>}
            {closed ? (
              <p className="note">{t.closed}</p>
            ) : (
              <>
                <p className="faint ws-outside-lead">{t.readOnly(where)}</p>
                <div className="ask-choices">
                  <Choice
                    label={busy === "adopt" ? t.working : t.adopt}
                    about={t.adoptAbout(where)}
                    tone="primary"
                    disabled={busy !== undefined || !s}
                    onClick={() => void act("adopt")}
                  />
                  <Choice
                    label={busy === "fork" ? t.working : t.fork}
                    about={t.forkAbout(where)}
                    disabled={busy !== undefined || !s}
                    onClick={() => void act("fork")}
                  />
                  <Choice
                    label={busy === "archive" ? t.working : t.archive}
                    about={t.archiveAbout(where)}
                    disabled={busy !== undefined || !s}
                    onClick={() => void act("archive")}
                  />
                </div>
              </>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}
