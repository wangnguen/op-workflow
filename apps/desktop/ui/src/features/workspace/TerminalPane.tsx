import { useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { vi } from "../../i18n/vi";
import { controlTerminal, errorMessage, openTerminal } from "../../ipc/client";
import type { TerminalAction, TerminalKind } from "../../ipc/types";
import { useTheme } from "../../shared/useTheme";

const tt = vi.workspace.terminal;
/** an open of the same console waits for the one before it (React mounts twice in dev) */
const connections = new Map<string, Promise<void>>();

function terminalTheme(el: HTMLElement) {
  const css = getComputedStyle(el);
  const value = (name: string) => css.getPropertyValue(name).trim();
  return {
    background: value("--bg"),
    foreground: value("--text"),
    cursor: value("--text"),
    selectionBackground: value("--fill-strong"),
  };
}

/** One console of a Workspace session: the session's Claude TUI or a shell in its repo
 *  (PLAN.md §12 B2). The process lives in the app; this view attaches to it, gets what it
 *  missed replayed, and lets go when unmounted (the process keeps running). */
export function TerminalPane({
  id, kind, live, onClose,
}: {
  id: string;
  kind: TerminalKind;
  /** the app runs this console's process now */
  live: boolean;
  onClose: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const instance = useRef<Terminal | null>(null);
  const addon = useRef<SearchAddon | null>(null);
  const close = useRef<() => Promise<void>>(async () => undefined);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string>();
  const [closing, setClosing] = useState(false);
  // it ran and has ended (exit in the TUI, the shell's own exit)
  const ran = useRef(false);
  if (live) ran.current = true;
  const exited = ran.current && !live;
  const { theme } = useTheme();

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const view = crypto.randomUUID();
    const key = `${id}:${kind}`;
    const term = new Terminal({
      fontFamily: "JetBrains Mono Variable, monospace",
      fontSize: 13,
      scrollback: 3000,
      cursorBlink: true,
      theme: terminalTheme(el),
    });
    instance.current = term;
    const fit = new FitAddon();
    const search = new SearchAddon();
    addon.current = search;
    term.loadAddon(fit);
    term.loadAddon(search);
    term.open(el);
    try {
      const webgl = new WebglAddon();
      term.loadAddon(webgl);
      webgl.onContextLoss(() => webgl.dispose());
    } catch {
      // xterm's DOM renderer stays active when WebGL is unavailable.
    }
    // hidden (another tab is shown): keep the size it had
    const refit = () => {
      if (el.clientWidth > 0 && el.clientHeight > 0) fit.fit();
    };
    refit();
    term.focus();

    let alive = true;
    let replaying = false;
    const send = (action: TerminalAction) => controlTerminal(id, kind, view, action);
    // xterm parses older output but does not answer queries from before it attached.
    let input = Promise.resolve();
    term.onData((data) => {
      if (replaying) return;
      input = input.then(() => connected).then(() => send({ kind: "write", data }))
        .catch((e) => { if (alive) setError(errorMessage(e)); });
    });
    const connected = (connections.get(key) ?? Promise.resolve()).then(async () => {
      if (!alive) return;
      await openTerminal(id, kind, view, [term.cols, term.rows], (frame) => {
        if (!alive) return;
        replaying = frame.replay;
        if (frame.skipped) term.writeln(`\r\n${tt.skipped}\r\n`);
        term.write(Uint8Array.from(frame.bytes), () => {
          replaying = false;
          if (alive) void send({ kind: "ack" }).catch(() => undefined);
        });
      });
    }).catch((e) => { if (alive) setError(errorMessage(e)); });
    connections.set(key, connected);
    void connected.then(() => {
      if (connections.get(key) === connected) connections.delete(key);
    });

    let resize = Promise.resolve();
    term.onResize(({ cols, rows }) => {
      resize = resize.then(() => connected).then(() => send({ kind: "resize", data: { cols, rows } }))
        .catch((e) => { if (alive) setError(errorMessage(e)); });
    });
    const observer = new ResizeObserver(refit);
    observer.observe(el);
    close.current = async () => {
      await connected;
      await send({ kind: "close" });
    };
    return () => {
      alive = false;
      observer.disconnect();
      instance.current = addon.current = null;
      term.dispose();
      void connected.then(() => send({ kind: "detach" })).catch(() => undefined);
    };
  }, [id, kind]);

  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      if (host.current && instance.current) instance.current.options.theme = terminalTheme(host.current);
    });
    return () => cancelAnimationFrame(frame);
  }, [theme]);

  return (
    <div className="ws-terminal">
      <div className="ws-terminal-bar">
        <input
          type="search"
          className="input ws-terminal-search"
          placeholder={tt.search}
          aria-label={tt.search}
          title={tt.searchHint}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            if (e.target.value) addon.current?.findNext(e.target.value, { incremental: true });
            else addon.current?.clearDecorations();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && query) {
              if (e.shiftKey) addon.current?.findPrevious(query);
              else addon.current?.findNext(query);
            }
          }}
        />
        <button
          className="btn btn--sm"
          disabled={closing}
          title={kind === "claude" ? tt.closeClaudeHint : tt.closeShellHint}
          onClick={() => {
            setClosing(true);
            void close.current().then(onClose).catch((e) => {
              setError(errorMessage(e));
              setClosing(false);
            });
          }}
        >
          {tt.close}
        </button>
      </div>
      {error && <p className="note note--error ws-terminal-note">{error}</p>}
      {exited && !error && <p className="note ws-terminal-note">{tt.exited}</p>}
      <div className="ws-terminal-host" ref={host} />
    </div>
  );
}
