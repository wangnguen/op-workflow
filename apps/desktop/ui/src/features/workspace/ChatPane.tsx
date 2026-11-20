import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import {
  answerSession,
  errorCode,
  errorMessage,
  hideSession,
  interruptSession,
  newSession,
  openSessionInVscode,
  sendToSession,
  setSessionMode,
  stopSession,
} from "../../ipc/client";
import type { AskAnswer, WorkspaceTask } from "../../ipc/types";
import { Markdown } from "../../shared/Markdown";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { ChatItemView } from "../chat/ChatItems";
import { ModeSelect } from "../settings/ModeSelect";
import type { Option } from "../settings/stepChoice";
import { AskCard } from "./AskCard";
import { repoName } from "./chatState";
import { useSessionChat } from "./useSessionChat";

const t = vi.workspace.chat;
const MODES = ["default", "acceptEdits", "plan", "auto"];
/** the input grows with its text up to this height, then scrolls */
const INPUT_MAX = 220;

interface Props {
  /** the session shown; absent = a task that has no session yet */
  sessionId?: string;
  /** the task of a draft (no session yet) */
  task?: WorkspaceTask;
  /** a session was created for the draft */
  onCreated: (id: string) => void;
  /** the session was taken off the list */
  onHidden: () => void;
  onChanged: () => void;
}

export function ChatPane({ sessionId, task, onCreated, onHidden, onChanged }: Props) {
  const { chat, error, reload, setSession } = useSessionChat(sessionId);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<string>();
  const [failure, setFailure] = useState<{ message: string; outside: boolean }>();
  // a draft's mode, set on the session when the first message creates it ("" = the CLI's own)
  const [draftMode, setDraftMode] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const input = useRef<HTMLTextAreaElement>(null);

  const session = chat?.session;
  const draft = !sessionId;
  const outside = session?.state === "outside";
  const running = session?.state === "running";
  const asks = chat?.asks ?? [];
  const count = (chat?.items.length ?? 0) + (chat?.partial ? 1 : 0) + asks.length;

  // a new session shown, or the draft of another task: start at the bottom, fresh input
  useEffect(() => {
    stick.current = true;
    setFailure(undefined);
    input.current?.focus();
  }, [sessionId, task?.wpId]);

  useEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [count, chat?.partial]);

  // the input fits its text (wrapped lines too), before paint so it never flickers
  useLayoutEffect(() => {
    const el = input.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(INPUT_MAX, el.scrollHeight)}px`;
  }, [text]);

  const act = async (what: string, run: () => Promise<unknown>) => {
    setBusy(what);
    setFailure(undefined);
    try {
      await run();
      onChanged();
    } catch (e) {
      setFailure({ message: errorMessage(e), outside: errorCode(e) === "session.outside" });
    } finally {
      setBusy(undefined);
    }
  };

  const send = (force = false) => {
    const message = text.trim();
    if (!message || busy) return;
    stick.current = true;
    void act("send", async () => {
      let id = sessionId;
      if (!id) {
        const created = await newSession(task ? { wpId: task.wpId } : {});
        id = created.id;
        if (draftMode) await setSessionMode(id, draftMode);
        onCreated(id);
      }
      const view = await sendToSession(id, message, force);
      setText("");
      if (sessionId) setSession(view);
    });
  };

  const answer = (askId: string, a: AskAnswer) =>
    sessionId && void act("answer", () => answerSession(sessionId, askId, a).then(setSession));

  const mode = session ? (session.mode ?? "") : draftMode;
  const modeOptions: Option[] = [
    ...(mode ? [] : [{ value: "", label: t.modeCli, about: t.modeCliAbout }]),
    ...MODES.map((m) => ({ value: m, label: t.modes[m] ?? m, about: t.modeAbout[m] })),
    ...(mode && !MODES.includes(mode) ? [{ value: mode, label: mode }] : []),
  ];

  const placeholder = outside ? t.placeholderOutside : running ? t.placeholderBusy : t.placeholder;
  // like the extension: while a turn runs and nothing is typed, the round button stops it
  const stopping = running && session !== undefined && !text.trim();

  return (
    <div className="ws-chat">
      <header className="ws-chat-head">
        {/* titles on the left, the session's buttons on the right: a draft has the same height,
            so the first message moves nothing */}
        <div className="ws-chat-col ws-chat-head-in">
          <div className="ws-chat-titles">
            <h2 title={session?.title}>{session?.title ?? task?.title ?? vi.workspace.title}</h2>
            <p className="faint">
              {(session?.taskId ?? task?.taskId) && <span className="chip">{session?.taskId ?? task?.taskId}</span>}
              <span className="mono">{repoName(session?.repo ?? task?.repo ?? "")}</span>
              {(session?.branch ?? task?.branch) && (
                <span className="ws-chat-branch">
                  <Icon name="branch" size={11} />
                  <span className="mono">{session?.branch ?? task?.branch}</span>
                </span>
              )}
              {session?.model && <span>{t.model(session.model)}</span>}
              {session && session.costUsd > 0 && <span title={t.costHint}>{t.cost(session.costUsd)}</span>}
              {session && (
                <span className={`ws-live${session.live ? " is-on" : ""}`} title={session.live ? t.liveHint : t.notLiveHint}>
                  {session.live ? t.live : t.notLive}
                </span>
              )}
            </p>
          </div>
          {session && (
            <div className="ws-chat-actions">
              {session.started && (
                <button
                  className="btn btn--sm"
                  title={t.openVscodeHint}
                  disabled={busy !== undefined || outside}
                  onClick={() => void act("vscode", () => openSessionInVscode(session.id))}
                >
                  <Icon name="external" size={12} />
                  {t.openVscode}
                </button>
              )}
              {session.live ? (
                <button
                  className="btn btn--sm btn--ghost"
                  title={t.endHint}
                  disabled={busy !== undefined}
                  onClick={() => void act("stop", () => stopSession(session.id).then(setSession))}
                >
                  <Icon name="pause" size={12} />
                  {t.end}
                </button>
              ) : (
                <button
                  className="btn btn--sm btn--ghost"
                  title={t.hideHint}
                  disabled={busy !== undefined}
                  onClick={() => void act("hide", () => hideSession(session.id).then(onHidden))}
                >
                  <Icon name="trash" size={12} />
                  {t.hide}
                </button>
              )}
            </div>
          )}
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
          {draft && <p className="ws-chat-empty dim">{task ? t.draftTask(task.taskId) : t.draftFree}</p>}
          {sessionId && !chat && !error && <p className="dim">{t.loading}</p>}
          {chat && chat.items.length === 0 && !session?.started && asks.length === 0 && !running && (
            <p className="ws-chat-empty dim">{task ? t.draftTask(task.taskId) : t.draftFree}</p>
          )}
          {chat && chat.dropped > 0 && <p className="faint">{t.dropped(chat.dropped)}</p>}
          {chat?.items.map((item, i) => <ChatItemView key={i} item={item} />)}
          {chat?.partial && (
            <div className="chat-item chat-text is-partial">
              <Markdown text={chat.partial} className="task-md" />
            </div>
          )}
          {running && !chat?.partial && asks.length === 0 && (
            <p className="ws-working faint">
              <StatusIcon state="running" />
              {chat && chat.items.length > 0 ? t.writing : t.starting}
            </p>
          )}
          {asks.map((a) => (
            <AskCard key={a.id} ask={a} busy={busy === "answer"} onAnswer={(x) => answer(a.id, x)} />
          ))}
          {session?.error && <p className="note note--error">{session.error}</p>}
        </div>
      </div>

      <footer className="ws-composer">
        <div className="ws-chat-col">
          {outside && session?.outside && (
            <div className="note ws-outside">
              <span>{t.outside(session.outside)}</span>
            </div>
          )}
          {failure && (
            <div className="note note--error ws-failure">
              <span>{failure.message}</span>
              {failure.outside && (
                <button className="btn btn--sm" title={t.forceHint} onClick={() => send(true)}>
                  {t.force}
                </button>
              )}
            </div>
          )}
          {/* one box like the extension's: the text on top, the mode and the round button under it */}
          <div className="ws-box">
            <textarea
              ref={input}
              className="ws-box-input"
              rows={1}
              value={text}
              placeholder={placeholder}
              aria-label={t.placeholder}
              disabled={busy === "send"}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  send(false);
                }
              }}
            />
            <div className="ws-box-bar">
              <div className="ws-mode" title={t.modeHint}>
                <span className="faint">{t.mode}</span>
                <ModeSelect
                  value={mode}
                  options={modeOptions}
                  label={t.mode}
                  disabled={busy !== undefined || outside || (!draft && !session)}
                  onChange={(m) => {
                    if (draft) setDraftMode(m ?? "");
                    else if (session && m) void act("mode", () => setSessionMode(session.id, m).then(setSession));
                  }}
                />
              </div>
              {stopping ? (
                <button
                  className="ws-send ws-send--stop"
                  title={t.stopHint}
                  aria-label={t.stop}
                  disabled={busy !== undefined}
                  onClick={() => session && void act("interrupt", () => interruptSession(session.id).then(reload))}
                >
                  <Icon name="stop" size={14} />
                </button>
              ) : (
                <button
                  className="ws-send"
                  title={t.send}
                  aria-label={t.send}
                  disabled={!text.trim() || busy !== undefined}
                  onClick={() => send(false)}
                >
                  <Icon name="arrowUp" size={15} />
                </button>
              )}
            </div>
          </div>
        </div>
      </footer>
    </div>
  );
}
