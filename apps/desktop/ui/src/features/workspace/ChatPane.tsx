import { Fragment, lazy, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { stepLabel, vi } from "../../i18n/vi";
import {
  answerSession,
  compareSessionFile,
  errorCode,
  errorMessage,
  getSessionDiff,
  getSessionScripts,
  hideSession,
  interruptSession,
  newSession,
  openSessionFile,
  openSessionInVscode,
  openSessionRepo,
  sendToSession,
  setSessionMode,
  stopSession,
} from "../../ipc/client";
import type { AskAnswer, CommentView, RepoScript, TerminalKind, WorkspaceTask } from "../../ipc/types";
import { formatDateTime } from "../../shared/format";
import { Markdown } from "../../shared/Markdown";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { ChatItemView } from "../chat/ChatItems";
import { CommentsTab } from "../task/CommentsTab";
import { BriefPane } from "./BriefPane";
import { DiffPanel } from "../task/DiffTab";
import { ModeSelect } from "../settings/ModeSelect";
import type { Option } from "../settings/stepChoice";
import { AskCard } from "./AskCard";
import { type Focus, repoName, stepItemIndex } from "./chatState";
import { AttachmentStrip, SuggestList, useAttachments, useSuggest } from "./ComposerExtras";
import { useSessionChat } from "./useSessionChat";
import { WorkspaceSteps } from "./WorkspaceSteps";

const TerminalPane = lazy(() => import("./TerminalPane").then((m) => ({ default: m.TerminalPane })));

const t = vi.workspace.chat;
const ws = vi.workspace;
const tt = vi.workspace.terminal;
const SHELLS = ["powerShell", "gitBash"] as const;
type Shell = (typeof SHELLS)[number];
const shellOptions: Option[] = SHELLS.map((s) => ({ value: s, label: tt.shells[s], about: tt.shellAbout[s] }));
/** the shell picked last, opened by itself the next time the Shell tab is shown */
const SHELL_KEY = "opw.workspace.shell";
function savedShell(): Shell {
  try {
    return window.localStorage.getItem(SHELL_KEY) === "gitBash" ? "gitBash" : "powerShell";
  } catch {
    return "powerShell";
  }
}
type View = "chat" | "claude" | "shell" | "changes" | "brief" | "comments";
const tc = vi.workspace.composer;
const tsc = vi.workspace.scripts;
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
  onOpenTask: (wpId: number, tab: string) => void;
  /** sent here from another screen: open this tab, land on a step's message or the question */
  focus?: Focus;
}

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;

export function ChatPane({ sessionId, task, onCreated, onHidden, onChanged, onOpenTask, focus }: Props) {
  const { chat, error, reload, setSession } = useSessionChat(sessionId);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<string>();
  const [failure, setFailure] = useState<{ message: string; outside: boolean }>();
  // a draft's mode, set on the session when the first message creates it ("" = the CLI's own)
  const [draftMode, setDraftMode] = useState("");
  const [view, setView] = useState<{ id?: string; tab: View }>();
  const tab = view?.id === sessionId ? (view?.tab ?? "chat") : "chat";
  const [shell, setShell] = useState<Shell>(savedShell);
  const [shellRequested, setShellRequested] = useState<string>();
  // a package script picked above the shell, typed into it once it is open
  const [command, setCommand] = useState<{ text: string; nonce: number }>();
  const [scripts, setScripts] = useState<RepoScript[]>();
  const [lastScript, setLastScript] = useState<string>();
  const [caret, setCaret] = useState(0);
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const input = useRef<HTMLTextAreaElement>(null);

  const session = chat?.session;
  const draft = !sessionId;
  const attach = useAttachments((message) => setFailure({ message, outside: false }));
  const suggest = useSuggest(sessionId, text, caret);
  const outside = session?.state === "outside";
  const terminal = session?.terminal === true;
  const running = session?.state === "running";
  const asks = chat?.asks ?? [];
  const count = (chat?.items.length ?? 0) + (chat?.partial ? 1 : 0) + asks.length;

  // a new session shown, or the draft of another task: start at the bottom, fresh input. The pane
  // stays mounted while a task's draft becomes its session (what was typed and picked is kept),
  // but another session of the same task starts clean.
  const shown = useRef(sessionId);
  useEffect(() => {
    stick.current = true;
    setFailure(undefined);
    if (shown.current !== undefined && shown.current !== sessionId) {
      setText("");
      setDraftMode("");
      attach.clear();
      setScripts(undefined);
      setCommand(undefined);
      setLastScript(undefined);
    }
    shown.current = sessionId;
    input.current?.focus();
  }, [sessionId, task?.wpId]);

  useEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [count, chat?.partial]);

  // sent here from another screen: open the tab, then (once the chat is read) scroll to the
  // message the step started with, or to the question; nothing to land on = the bottom as usual
  const landing = useRef<Focus | undefined>(undefined);
  useEffect(() => {
    if (!focus) return;
    landing.current = focus;
    setView({ id: sessionId, tab: focus.view ?? "chat" });
  }, [focus?.at]);
  useEffect(() => {
    const f = landing.current;
    if (!f) return;
    if ((f.view ?? "chat") !== "chat" || (!f.step && !f.ask)) {
      landing.current = undefined;
      return;
    }
    if (!chat) return;
    landing.current = undefined;
    const el = list.current;
    const i = f.step ? stepItemIndex(chat.items, f.step) : -1;
    const target = f.ask
      ? el?.querySelector<HTMLElement>(".ask")
      : i >= 0
        ? el?.querySelector<HTMLElement>(`[data-item="${i}"]`)
        : null;
    if (!target) return;
    stick.current = false;
    requestAnimationFrame(() => {
      target.scrollIntoView({ block: f.ask ? "center" : "start", behavior: reducedMotion() ? "auto" : "smooth" });
      // a short glow shows where you landed
      target.classList.remove("is-target");
      void target.offsetWidth;
      target.classList.add("is-target");
    });
  }, [chat, focus?.at]);

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
    const images = attach.images;
    if ((!message && images.length === 0) || busy || terminal) return;
    stick.current = true;
    void act("send", async () => {
      let id = sessionId;
      if (!id) {
        const created = await newSession(task ? { wpId: task.wpId } : {});
        id = created.id;
        if (draftMode) await setSessionMode(id, draftMode);
        onCreated(id);
      }
      const view = await sendToSession(id, message, force, images);
      setText("");
      attach.clear();
      if (sessionId) setSession(view);
    });
  };

  const switchTab = (next: View) => {
    if (next === "chat" || sessionId) {
      setView({ id: sessionId, tab: next });
      return;
    }
    void act("terminal", async () => {
      const created = await newSession(task ? { wpId: task.wpId } : {});
      if (draftMode) await setSessionMode(created.id, draftMode);
      setView({ id: created.id, tab: next });
      onCreated(created.id);
    });
  };
  // a terminal stays mounted (hidden) while it runs, so going back to it keeps its screen
  const claudeShown = !!sessionId && (tab === "claude" || terminal);
  const shellKind: TerminalKind | undefined =
    session?.shell ?? (shellRequested !== undefined && shellRequested === sessionId ? shell : undefined);
  useEffect(() => {
    if (tab !== "shell" || !sessionId || scripts) return;
    let alive = true;
    getSessionScripts(sessionId).then((s) => alive && setScripts(s)).catch(() => alive && setScripts([]));
    return () => {
      alive = false;
    };
  }, [tab, sessionId, scripts]);
  // the Shell tab shows a shell at once (the kind picked last), no empty page with a button
  useEffect(() => {
    if (tab === "shell" && sessionId && !shellKind) setShellRequested(sessionId);
  }, [tab, sessionId, shellKind]);
  const runScript = (script: RepoScript) => {
    if (!sessionId) return;
    if (!shellKind) setShellRequested(sessionId);
    setCommand({ text: script.run, nonce: Date.now() });
    setLastScript(script.name);
  };
  // another kind of shell: the open one was closed (by the terminal), the new one opens once
  // the session no longer reports the old one
  const switchShell = (next: Shell) => {
    setShell(next);
    setCommand(undefined);
    try {
      window.localStorage.setItem(SHELL_KEY, next);
    } catch {
      // only a convenience: the next Shell tab opens PowerShell again
    }
    reload();
    onChanged();
  };
  const loadDiff = useCallback(() => getSessionDiff(sessionId ?? ""), [sessionId]);
  const openFile = useCallback(
    (path: string, line?: number) => openSessionFile(sessionId ?? "", path, line),
    [sessionId],
  );
  const compareFile = useCallback(
    (rev: string, path: string, oldPath?: string) => compareSessionFile(sessionId ?? "", rev, path, oldPath),
    [sessionId],
  );
  // "Hỏi Claude" on a comment: it goes into the chat box, quoted, and the chat comes back
  const askAbout = (c: CommentView, who: string, body: string) => {
    const quoted = body.trim().split("\n").map((l) => `> ${l}`).join("\n");
    const head = t.askAbout(who, formatDateTime(c.createdAt));
    const next = text.trim() ? `${head}\n${quoted}\n\n${text}` : `${head}\n${quoted}\n\n`;
    setText(next);
    setView({ id: sessionId, tab: "chat" });
    requestAnimationFrame(() => {
      input.current?.focus();
      input.current?.setSelectionRange(next.length, next.length);
      setCaret(next.length);
    });
  };
  const closeTerminal = (kind: TerminalKind) => {
    if (kind !== "claude") {
      setShellRequested(undefined);
      setCommand(undefined);
    }
    setView({ id: sessionId, tab: "chat" });
    reload();
    onChanged();
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
  const stopping = running && session !== undefined && !text.trim() && attach.images.length === 0;
  const pickSuggestion = (i: number) => {
    const next = suggest.apply(i);
    if (!next) return;
    setText(next.text);
    setCaret(next.caret);
    requestAnimationFrame(() => {
      input.current?.focus();
      input.current?.setSelectionRange(next.caret, next.caret);
    });
  };

  return (
    <div className="ws-chat">
      {task && (
        <WorkspaceSteps key={task.wpId} wpId={task.wpId} session={session}
          beforeRun={async () => {
            if (!sessionId) {
              const created = await newSession({ wpId: task.wpId });
              if (draftMode) await setSessionMode(created.id, draftMode);
              onCreated(created.id);
            }
            setView(undefined);
          }}
          onChanged={onChanged} onOpenTask={onOpenTask} onSession={onCreated} />
      )}
      <div className="ws-tabs" role="group" aria-label={ws.views}>
        <button className="btn btn--sm" aria-pressed={tab === "chat"} onClick={() => switchTab("chat")}>
          {ws.tabChat}
        </button>
        <button className="btn btn--sm" aria-pressed={tab === "claude"} title={ws.tabClaudeHint}
          disabled={busy !== undefined} onClick={() => switchTab("claude")}>
          {ws.tabClaude}
          {terminal && <span className="ws-tab-live" aria-hidden="true" />}
        </button>
        <button className="btn btn--sm" aria-pressed={tab === "shell"} title={ws.tabShellHint}
          disabled={busy !== undefined} onClick={() => switchTab("shell")}>
          {ws.tabShell}
          {session?.shell && <span className="ws-tab-live" aria-hidden="true" />}
        </button>
        <button className="btn btn--sm" aria-pressed={tab === "changes"}
          title={sessionId ? ws.tabChangesHint : ws.needsSession}
          disabled={!sessionId} onClick={() => switchTab("changes")}>
          {ws.tabChanges}
        </button>
        {task && (
          <button className="btn btn--sm" aria-pressed={tab === "brief"} title={ws.tabBriefHint}
            onClick={() => setView({ id: sessionId, tab: "brief" })}>
            {ws.tabBrief}
          </button>
        )}
        {task && (
          <button className="btn btn--sm" aria-pressed={tab === "comments"} title={ws.tabCommentsHint}
            onClick={() => setView({ id: sessionId, tab: "comments" })}>
            {ws.tabComments}
          </button>
        )}
        {sessionId && (
          <button className="btn btn--sm btn--ghost ws-tabs-end" title={t.openRepoHint}
            disabled={busy !== undefined} onClick={() => void act("repo", () => openSessionRepo(sessionId))}>
            <Icon name="folder" size={12} />
            {t.openRepo}
          </button>
        )}
      </div>
      {tab === "shell" && sessionId && scripts && scripts.length > 0 && (
        // one row of names (the command in the tooltip); a pick runs in the shell below
        <div className="ws-scripts" role="group" aria-label={tsc.title}>
          <span className="faint ws-scripts-label">{tsc.label}</span>
          {scripts.map((sc) => (
            <button
              key={sc.name}
              className={`btn btn--sm ws-script${lastScript === sc.name ? " is-last" : ""}`}
              title={tsc.runs(sc.command)}
              onClick={() => runScript(sc)}
            >
              {sc.name}
            </button>
          ))}
          <span className="faint ws-scripts-about">{tsc.about}</span>
        </div>
      )}
      {claudeShown && (
        <div className="ws-term-wrap" hidden={tab !== "claude"}>
          <Suspense fallback={<p className="dim ws-term-wait">{tt.opening}</p>}>
            <TerminalPane key={`${sessionId}:claude`} id={sessionId} kind="claude" live={terminal}
              onClose={() => closeTerminal("claude")} />
          </Suspense>
        </div>
      )}
      {sessionId && shellKind && (
        <div className="ws-term-wrap" hidden={tab !== "shell"}>
          <Suspense fallback={<p className="dim ws-term-wait">{tt.opening}</p>}>
            <TerminalPane key={`${sessionId}:${shellKind}`} id={sessionId} kind={shellKind}
              live={session?.shell === shellKind} command={command} onClose={() => closeTerminal(shellKind)}
              kinds={{ value: shellKind, options: shellOptions, label: tt.shellKind,
                onSwitched: (k) => switchShell(k === "gitBash" ? "gitBash" : "powerShell") }} />
          </Suspense>
        </div>
      )}
      {tab === "changes" && sessionId && (
        <div className="ws-tool-wrap">
          <DiffPanel key={sessionId} load={loadDiff} findings={[]} onOpenFile={openFile} onCompare={compareFile} />
        </div>
      )}
      {tab === "brief" && task && (
        <div className="ws-tool-wrap">
          <BriefPane key={task.wpId} wpId={task.wpId} onOpenTask={onOpenTask} />
        </div>
      )}
      {tab === "comments" && task && (
        <div className="ws-tool-wrap">
          <CommentsTab key={task.wpId} wpId={task.wpId} running={running} onChanged={onChanged} onAsk={askAbout} />
        </div>
      )}
      <div className="ws-chat-body" hidden={tab !== "chat"}>
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
              {session?.contextPercent !== undefined && (
                <span title={tt.contextHint}>{tt.context(session.contextPercent)}</span>
              )}
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
              {session.started && !terminal && (
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
              {session.live && (
                <button
                  className="btn btn--sm btn--ghost"
                  title={t.endHint}
                  disabled={busy !== undefined || terminal || !!session.shell}
                  onClick={() => void act("stop", () => stopSession(session.id).then(setSession))}
                >
                  <Icon name="pause" size={12} />
                  {t.end}
                </button>
              )}
              {/* like the extension: off the list, back from "Đã lưu trữ"; not in the middle of a
                  turn or while a terminal of it is open */}
              <button
                className="btn btn--sm btn--ghost"
                title={running ? t.archiveBusy : terminal || session.shell ? t.archiveTerminal : t.archiveHint}
                disabled={busy !== undefined || running || outside || terminal || !!session.shell}
                onClick={() => void act("hide", () => hideSession(session.id).then(onHidden))}
              >
                <Icon name="archive" size={12} />
                {t.archive}
              </button>
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
          {chat?.items.map((item, i) => (
            <Fragment key={i}>
              {item.step && (
                <span className="chip ws-step-label" data-item={i}>
                  {stepLabel[item.step]}
                </span>
              )}
              <ChatItemView item={item} />
            </Fragment>
          ))}
          {chat?.partial && (
            <div className="chat-item chat-text is-partial">
              <Markdown text={chat.partial} className="task-md" />
            </div>
          )}
          {running && !terminal && !chat?.partial && asks.length === 0 && (
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
          {terminal && <p className="note">{tt.chatReadOnly}</p>}
          {session?.forkOf && <p className="note">{t.forkNote}</p>}
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
          {!terminal && <div className="ws-box" onDrop={attach.onDrop} onDragOver={(e) => e.preventDefault()}>
            {suggest.open && (
              <SuggestList kind={suggest.kind} items={suggest.items} active={suggest.active}
                onHover={suggest.setActive} onPick={(i) => pickSuggestion(i)} />
            )}
            <AttachmentStrip images={attach.images} onRemove={attach.remove} />
            <textarea
              ref={input}
              className="ws-box-input"
              rows={1}
              value={text}
              placeholder={placeholder}
              aria-label={t.placeholder}
              title={tc.hint}
              disabled={busy === "send"}
              onChange={(e) => {
                setText(e.target.value);
                setCaret(e.target.selectionStart);
              }}
              onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
              onPaste={attach.onPaste}
              onKeyDown={(e) => {
                if (suggest.open) {
                  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                    e.preventDefault();
                    suggest.move(e.key === "ArrowDown" ? 1 : -1);
                    return;
                  }
                  if ((e.key === "Enter" || e.key === "Tab") && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    pickSuggestion(suggest.active);
                    return;
                  }
                  if (e.key === "Escape") {
                    e.preventDefault();
                    suggest.close();
                    return;
                  }
                }
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
                  disabled={(!text.trim() && attach.images.length === 0) || busy !== undefined}
                  onClick={() => send(false)}
                >
                  <Icon name="arrowUp" size={15} />
                </button>
              )}
            </div>
          </div>}
        </div>
      </footer>
      </div>
    </div>
  );
}
