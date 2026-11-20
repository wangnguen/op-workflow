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

/** Reads theme tokens as #rrggbb (a token may be any CSS colour, color-mix included). */
function tokenReader(el: HTMLElement) {
  const probe = document.createElement("span");
  probe.style.display = "none";
  el.appendChild(probe);
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  const read = (name: string) => {
    probe.style.color = `var(${name})`;
    const css = getComputedStyle(probe).color;
    if (!ctx) return css;
    ctx.clearRect(0, 0, 1, 1);
    ctx.fillStyle = "#000";
    ctx.fillStyle = css;
    ctx.fillRect(0, 0, 1, 1);
    const px = ctx.getImageData(0, 0, 1, 1).data;
    return `#${[px[0], px[1], px[2]].map((v) => (v ?? 0).toString(16).padStart(2, "0")).join("")}`;
  };
  return { read, done: () => probe.remove() };
}

const luminance = (hex: string) => {
  const [r = 0, g = 0, b = 0] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

/** The console in the app's theme: its surface and text, the 16 ANSI colours from the theme's
 *  hues (black / white swap on a light theme so "black" text still reads). */
function terminalTheme(el: HTMLElement) {
  const { read, done } = tokenReader(el);
  const bg = read("--bg");
  const light = luminance(bg) > 0.5;
  const [text, dim, faint, elev] = ["--text", "--text-dim", "--text-faint", "--bg-elev"].map(read);
  const hue = (name: string) => read(`--hue-${name}`);
  const [red, green, yellow, blue, purple, teal, orange, pink] =
    ["red", "green", "yellow", "blue", "purple", "teal", "orange", "pink"].map(hue);
  const theme = {
    background: bg,
    foreground: text,
    cursor: text,
    cursorAccent: bg,
    selectionBackground: `${read("--accent")}55`,
    black: light ? text : elev,
    brightBlack: light ? dim : faint,
    white: light ? faint : dim,
    brightWhite: light ? elev : text,
    red, green, yellow, blue, magenta: purple, cyan: teal,
    brightRed: orange, brightGreen: green, brightYellow: yellow, brightBlue: blue,
    brightMagenta: pink, brightCyan: teal,
  };
  done();
  return theme;
}

/** One console of a Workspace session: the session's Claude TUI or a shell in its repo
 *  (PLAN.md §12 B2). The process lives in the app; this view attaches to it, gets what it
 *  missed replayed, and lets go when unmounted (the process keeps running). */
export function TerminalPane({
  id, kind, live, command, onClose,
}: {
  id: string;
  kind: TerminalKind;
  /** the app runs this console's process now */
  live: boolean;
  /** a line to type into the console (a script picked above it); a new `nonce` types it again */
  command?: { text: string; nonce: number };
  onClose: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const instance = useRef<Terminal | null>(null);
  const addon = useRef<SearchAddon | null>(null);
  const close = useRef<() => Promise<void>>(async () => undefined);
  const type = useRef<(data: string) => void>(() => undefined);
  // the nonce of the script line typed last (a view that let go types it again when it comes back)
  const typed = useRef<number>(undefined);
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
      // colours a program picks itself (Claude Code's own palette) still read on a light theme
      minimumContrastRatio: 4.5,
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
    const write = (data: string) => {
      input = input.then(() => connected).then(() => send({ kind: "write", data }))
        .catch((e) => { if (alive) setError(errorMessage(e)); });
    };
    type.current = write;
    term.onData((data) => {
      if (!replaying) write(data);
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
      typed.current = undefined;
      term.dispose();
      void connected.then(() => send({ kind: "detach" })).catch(() => undefined);
    };
  }, [id, kind]);

  // a script picked above the shell: typed once, after the console is attached, then Enter.
  // Whatever was half typed on the line goes first (Esc in PowerShell, Ctrl+E Ctrl+U in bash),
  // so it does not run along. Enter is a carriage return, as the keyboard sends it: a line feed
  // is "one more line" to PowerShell (it shows >> and waits), so the command would not run.
  useEffect(() => {
    if (!command || typed.current === command.nonce) return;
    typed.current = command.nonce;
    const clearLine = kind === "gitBash" ? "\x05\x15" : kind === "powerShell" ? "\x1b" : "";
    type.current(`${clearLine}${command.text}\r`);
    instance.current?.focus();
  }, [command, kind]);

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
