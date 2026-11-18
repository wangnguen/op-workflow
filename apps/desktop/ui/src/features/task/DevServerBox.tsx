import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import type { DevCommand, DevServer, FlowAction, TaskDetail } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";

const t = vi.flow.devServer;
/** At most this many commands (the Rust side checks it too). */
export const MAX_COMMANDS = 5;

/** A row being edited, with a key that stays when rows above it go. */
type Row = DevCommand & { key: number };
type Draft = { url: string; commands: Row[] };
let nextKey = 1;

const row = (c: DevCommand): Row => ({ ...c, key: nextKey++ });
const emptyRow = (): Row => row({ command: "", dir: "", wait: "" });
/** At least one row to type in (a row left empty is not saved). */
const toDraft = (s: DevServer): Draft => ({
  url: s.url,
  commands: s.commands.length ? s.commands.map(row) : [emptyRow()],
});
const blank = (c: DevCommand) => !c.command.trim() && !c.wait.trim();

/** What the app saves: trimmed, rows left empty dropped. */
export function tidy(s: DevServer): DevServer {
  return {
    url: s.url.trim(),
    commands: s.commands
      .filter((c) => !blank(c))
      .map((c) => ({ command: c.command.trim(), dir: c.dir.trim(), wait: c.wait.trim() })),
  };
}

const URL_RE = /^https?:\/\/[^/\s:]+(:\d+)?(\/\S*)?$/i;

/** A URL the app can wait for: http(s) with a host. Empty = no dev server. */
export function urlProblem(url: string): string | undefined {
  const u = url.trim();
  if (!u) return undefined;
  return URL_RE.test(u) ? undefined : t.badUrl;
}

/** What a command is waited for: a port on localhost (3000, :3000) or a URL. */
export function waitOk(wait: string): boolean {
  const w = wait.trim();
  const port = w.replace(/^:/, "");
  if (/^\d+$/.test(port)) return Number(port) > 0 && Number(port) < 65536;
  return URL_RE.test(w);
}

/** The first thing to fix before the app can use what is typed (numbers as on screen). Only
 *  the last command may leave Chờ empty (= wait for the URL). */
export function serverProblem(s: DevServer): string | undefined {
  const url = urlProblem(s.url);
  if (url) return url;
  const filled = s.commands.map((c, i) => ({ c, n: i + 1 })).filter(({ c }) => !blank(c));
  if (filled.length && !s.url.trim()) return t.needUrl;
  for (const [i, { c, n }] of filled.entries()) {
    if (!c.command.trim()) return t.noCommand(n);
    if (!c.wait.trim()) {
      if (i < filled.length - 1) return t.needWait(n);
    } else if (!waitOk(c.wait)) return t.badWait(n);
  }
  return undefined;
}

const same = (a: DevServer, b: DevServer) => JSON.stringify(tidy(a)) === JSON.stringify(tidy(b));

/** Test trình duyệt: how the app runs this repo before the AI opens it in the test's own Chrome
 *  window: the commands to run (one `pnpm dev`, or an API then the web…), top to bottom, and the
 *  URL to open. Prefilled with what is in use, else what the analysis proposed; the app runs
 *  nothing until the user saves it (then it is remembered for the repo). */
export function DevServerBox({
  detail: d,
  disabled,
  onAction,
}: {
  detail: TaskDetail;
  disabled: boolean;
  onAction: (a: FlowAction) => void;
}) {
  const proposed = d.proposals?.find((p) => p.step === "browserTest")?.devServer;
  const current = d.devServer;
  const start: DevServer = current ?? proposed ?? { url: "", commands: [] };
  const [draft, setDraft] = useState<Draft>(() => toDraft(start));
  // what is in use changed (saved, cleared, a new analysis): show it (not on mount, so the rows
  // keep their keys and do not fade in twice)
  const startKey = JSON.stringify(tidy(start));
  const shown = useRef(startKey);
  useEffect(() => {
    if (shown.current === startKey) return;
    shown.current = startKey;
    setDraft(toDraft(start));
  }, [startKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const problem = serverProblem(draft);
  const changed = current ? !same(draft, current) : draft.url.trim() !== "";
  const ready = !disabled && changed && !problem;
  const save = () => onAction({ type: "setDevServer", ...tidy(draft) });
  const onEnter = (e: React.KeyboardEvent) => e.key === "Enter" && ready && save();
  const rows = draft.commands;
  // every change builds on the latest draft (several changes in one tick, autofill…)
  const setRow = (key: number, field: keyof DevCommand) => (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    setDraft((d) => ({ ...d, commands: d.commands.map((r) => (r.key === key ? { ...r, [field]: value } : r)) }));
  };
  const setUrl = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    setDraft((d) => ({ ...d, url: value }));
  };
  const addRow = () => setDraft((d) => ({ ...d, commands: [...d.commands, emptyRow()] }));
  const removeRow = (key: number) => setDraft((d) => ({ ...d, commands: d.commands.filter((r) => r.key !== key) }));
  // commands run top to bottom: move one up to run it earlier (an API before the web)
  const moveUp = (key: number) =>
    setDraft((d) => {
      const i = d.commands.findIndex((r) => r.key === key);
      if (i < 1) return d;
      const commands = [...d.commands];
      const [moved] = commands.splice(i, 1);
      if (!moved) return d;
      commands.splice(i - 1, 0, moved);
      return { ...d, commands };
    });
  const status = current
    ? current.commands.length === 0
      ? t.running
      : d.devServerFromRepo
        ? t.fromRepo
        : t.inUse
    : proposed
      ? t.proposed
      : t.none;

  return (
    <div className="dev-server" aria-label={t.title}>
      <div className="dev-server-head">
        <span className="faint">{t.title}</span>
        <span className={current ? "dev-server-status dev-server-status--on" : "dev-server-status"}>{status}</span>
      </div>
      <span className="dev-server-list-note faint">{t.listNote}</span>
      {rows.map((r, i) => (
        <div key={r.key} className="dev-server-fields dev-server-row">
          <label className="dev-server-field">
            <span className="faint">{rows.length === 1 ? t.command : t.commandN(i + 1)}</span>
            <input className="input mono" value={r.command} placeholder="npm run dev" disabled={disabled} onChange={setRow(r.key, "command")} onKeyDown={onEnter} />
          </label>
          <label className="dev-server-field">
            <span className="faint">{t.dir}</span>
            <input className="input mono" value={r.dir} placeholder={t.dirPlaceholder} disabled={disabled} onChange={setRow(r.key, "dir")} onKeyDown={onEnter} />
          </label>
          <label className="dev-server-field">
            <span className="faint">{t.wait}</span>
            <input
              className="input mono"
              value={r.wait}
              placeholder={i === rows.length - 1 ? t.waitLast : "3000"}
              disabled={disabled}
              aria-invalid={r.wait.trim() && !waitOk(r.wait) ? true : undefined}
              onChange={setRow(r.key, "wait")}
              onKeyDown={onEnter}
            />
          </label>
          <span className="dev-server-row-actions">
            {/* the first row keeps the place of its (hidden) up button, so the x buttons line up */}
            <button
              className={i === 0 ? "btn btn--sm btn--ghost btn--icon dev-server-up dev-server-up--first" : "btn btn--sm btn--ghost btn--icon dev-server-up"}
              disabled={disabled || i === 0}
              aria-hidden={i === 0 ? true : undefined}
              tabIndex={i === 0 ? -1 : undefined}
              aria-label={t.moveUp}
              title={t.moveUp}
              onClick={() => moveUp(r.key)}
            >
              <Icon name="chevron" size={13} />
            </button>
            <button className="btn btn--sm btn--ghost btn--icon" disabled={disabled} aria-label={t.remove} title={t.remove} onClick={() => removeRow(r.key)}>
              <Icon name="close" size={13} />
            </button>
          </span>
        </div>
      ))}
      <div className="dev-server-fields">
        <label className="dev-server-field dev-server-field--url">
          <span className="faint">{t.url}</span>
          <input
            className="input mono"
            value={draft.url}
            placeholder="http://localhost:5173"
            disabled={disabled}
            aria-invalid={urlProblem(draft.url) ? true : undefined}
            onChange={setUrl}
            onKeyDown={onEnter}
          />
        </label>
      </div>
      <div className="dev-server-foot">
        <span className={problem ? "dev-server-note dev-server-note--warn" : "dev-server-note faint"}>{problem ?? t.note}</span>
        <button className="btn btn--ghost btn--sm" disabled={disabled || rows.length >= MAX_COMMANDS} title={t.addHint} onClick={addRow}>
          {t.add}
        </button>
        {current && (
          <button
            className="btn btn--ghost btn--sm"
            disabled={disabled}
            title={t.clearHint}
            onClick={() => onAction({ type: "setDevServer", url: "", commands: [] })}
          >
            {t.clear}
          </button>
        )}
        <button className="btn btn--sm" disabled={!ready} onClick={save}>
          {current ? t.save : t.use}
        </button>
      </div>
    </div>
  );
}
