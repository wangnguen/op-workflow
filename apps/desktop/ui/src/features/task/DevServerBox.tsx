import { useEffect, useState } from "react";
import { vi } from "../../i18n/vi";
import type { DevServer, FlowAction, TaskDetail } from "../../ipc/types";

const t = vi.flow.devServer;
const EMPTY: DevServer = { command: "", url: "", dir: "" };

/** A URL the app can wait for: http(s) with a host. Empty = no dev server. */
export function urlProblem(url: string): string | undefined {
  const u = url.trim();
  if (!u) return undefined;
  return /^https?:\/\/[^/\s:]+(:\d+)?(\/\S*)?$/i.test(u) ? undefined : t.badUrl;
}

const same = (a: DevServer, b: DevServer) =>
  a.command.trim() === b.command.trim() && a.url.trim() === b.url.trim() && a.dir.trim() === b.dir.trim();

/** Test trình duyệt: how the app runs this repo's dev server before the AI opens it in the
 *  test's own Chrome window. Prefilled with what is in use, else what the analysis proposed; the
 *  app runs nothing until the user saves it (then it is remembered for the repo). */
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
  const start = current ?? proposed ?? EMPTY;
  const [draft, setDraft] = useState<DevServer>(start);
  // what is in use changed (saved, cleared, a new analysis): show it
  const startKey = `${start.command}\n${start.url}\n${start.dir}`;
  useEffect(() => setDraft(start), [startKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const problem = urlProblem(draft.url);
  const changed = current ? !same(draft, current) : draft.url.trim() !== "";
  const save = () => onAction({ type: "setDevServer", command: draft.command.trim(), url: draft.url.trim(), dir: draft.dir.trim() });
  const set = (key: keyof DevServer) => (e: React.ChangeEvent<HTMLInputElement>) => setDraft({ ...draft, [key]: e.target.value });
  const status = current
    ? d.devServerFromRepo
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
      <div className="dev-server-fields">
        <label className="dev-server-field dev-server-field--command">
          <span className="faint">{t.command}</span>
          <input
            className="input mono"
            value={draft.command}
            placeholder="npm run dev"
            disabled={disabled}
            onChange={set("command")}
            onKeyDown={(e) => e.key === "Enter" && changed && !problem && save()}
          />
        </label>
        <label className="dev-server-field">
          <span className="faint">{t.dir}</span>
          <input
            className="input mono"
            value={draft.dir}
            placeholder={t.dirPlaceholder}
            disabled={disabled}
            onChange={set("dir")}
            onKeyDown={(e) => e.key === "Enter" && changed && !problem && save()}
          />
        </label>
        <label className="dev-server-field dev-server-field--url">
          <span className="faint">{t.url}</span>
          <input
            className="input mono"
            value={draft.url}
            placeholder="http://localhost:5173"
            disabled={disabled}
            aria-invalid={problem ? true : undefined}
            onChange={set("url")}
            onKeyDown={(e) => e.key === "Enter" && changed && !problem && save()}
          />
        </label>
      </div>
      <div className="dev-server-foot">
        <span className={problem ? "dev-server-note dev-server-note--warn" : "dev-server-note faint"}>{problem ?? t.note}</span>
        {current && (
          <button className="btn btn--ghost btn--sm" disabled={disabled} title={t.clearHint} onClick={() => onAction({ type: "setDevServer", command: "", url: "", dir: "" })}>
            {t.clear}
          </button>
        )}
        <button className="btn btn--sm" disabled={disabled || !changed || !!problem} onClick={save}>
          {current ? t.save : t.use}
        </button>
      </div>
    </div>
  );
}
