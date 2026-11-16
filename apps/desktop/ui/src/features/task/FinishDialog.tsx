// "Đã push xong" (button or a drop on "Đã xong"): `git fetch`, then is the task branch on
// origin? Pushed at the push summary → the task is done at once and the dialog says "Nhớ tạo PR".
// Not pushed → what is missing, with "Kiểm tra lại" and "Vẫn đánh dấu xong". Before the push
// summary → it also says the steps left are skipped. The app never pushes.
import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, flowAction, pushCheck } from "../../ipc/client";
import type { PushCheck, TaskDetail } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import "./flowDialogs.css";

const p = vi.push;

interface Props {
  wpId: number;
  taskId: string;
  branch?: string;
  /** the task waits at "Chờ push" (else marking it done skips the steps left) */
  atPush: boolean;
  /** step the task is at, for the "skipped" warning */
  stepName?: string;
  onClose: () => void;
  /** marked done; `pushed` = the check found the branch on origin */
  onFinished: (detail: TaskDetail, pushed: boolean) => void;
}

type State =
  | { kind: "checking" }
  | { kind: "checked"; check: PushCheck }
  | { kind: "failed"; error: string }
  | { kind: "finishing"; check?: PushCheck }
  | { kind: "done"; check?: PushCheck };

/** What the check found, one short line each. */
function missing(c: PushCheck): string[] {
  const lines: string[] = [];
  if (!c.remote) lines.push(p.noRemote(c.branch));
  if (c.ahead > 0) lines.push(p.ahead(c.ahead));
  if (c.behind > 0) lines.push(p.behind(c.behind));
  if (c.uncommitted > 0) lines.push(p.uncommitted(c.uncommitted));
  return lines;
}

export function FinishDialog({ wpId, taskId, branch, atPush, stepName, onClose, onFinished }: Props) {
  const [state, setState] = useState<State>({ kind: "checking" });
  const [error, setError] = useState<string>();
  const alive = useRef(true);
  const checked = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const finish = (check: PushCheck | undefined, forced: boolean) => {
    setError(undefined);
    setState({ kind: "finishing", check });
    flowAction(wpId, { type: "finish", forced })
      .then((detail) => {
        if (!alive.current) return;
        const pushed = check?.pushed === true;
        onFinished(detail, pushed);
        if (pushed) setState({ kind: "done", check });
        else onClose();
      })
      .catch((e: unknown) => {
        if (!alive.current) return;
        setError(errorMessage(e));
        setState(check ? { kind: "checked", check } : { kind: "failed", error: p.checkFailed });
      });
  };

  const check = () => {
    setError(undefined);
    setState({ kind: "checking" });
    pushCheck(wpId)
      .then((c) => {
        if (!alive.current) return;
        // pushed and nothing else to do: done at once, the dialog only reminds the PR
        if (c.pushed && atPush) finish(c, false);
        else setState({ kind: "checked", check: c });
      })
      .catch((e: unknown) => alive.current && setState({ kind: "failed", error: errorMessage(e) }));
  };

  // once per open (StrictMode runs effects twice in dev; the ref keeps it to one fetch)
  useEffect(() => {
    if (checked.current) return;
    checked.current = true;
    check();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const busy = state.kind === "checking" || state.kind === "finishing";
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      if (!busy || state.kind === "checking") onClose();
    };
    // capture: the task window behind listens on the document too
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, state.kind, onClose]);

  const c = state.kind === "checked" || state.kind === "finishing" || state.kind === "done" ? state.check : undefined;
  const pushed = c?.pushed === true;

  return (
    <div className="scrim flow-scrim" onPointerDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="sheet flow-sheet" role="dialog" aria-modal="true" aria-labelledby="finish-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name={state.kind === "done" ? "check" : "branch"} size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="finish-title">{state.kind === "done" ? p.doneTitle(taskId) : p.title}</h2>
            <p>
              <span className="mono">{taskId}</span>
              {branch && (
                <>
                  {" · "}
                  <span className="mono">{branch}</span>
                </>
              )}
            </p>
          </div>
        </header>

        <div className="sheet-body flow-body">
          {!atPush && state.kind !== "done" && stepName && (
            <p className="note note--warn">
              <Icon name="alert" size={13} />
              {p.skipSteps(stepName)}
            </p>
          )}

          {state.kind === "checking" && (
            <p className="flow-line flow-line--busy" role="status">
              <Icon name="refresh" size={14} className="is-spinning" />
              {p.checking(branch ?? taskId)}
            </p>
          )}

          {state.kind === "failed" && (
            <p className="note note--error">
              <Icon name="alert" size={13} />
              <span>
                <strong>{p.checkFailed}:</strong> {state.error}
              </span>
            </p>
          )}

          {c && pushed && (
            <p className="flow-line flow-line--ok">
              <StatusIcon state="done" size={15} />
              <span>
                {p.pushed(c.branch, c.head)}
                {state.kind === "done" && <strong> {p.rememberPr}</strong>}
              </span>
            </p>
          )}
          {c && !pushed && (
            <div className="flow-line flow-line--warn">
              <StatusIcon state="failed" size={15} />
              <div>
                <p>{p.notPushed}</p>
                <ul className="flow-list">
                  {missing(c).map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ul>
              </div>
            </div>
          )}
          {c && pushed && c.uncommitted > 0 && (
            <p className="note">
              <Icon name="info" size={13} />
              {p.uncommitted(c.uncommitted)}
            </p>
          )}
          {c?.otherBranch && (
            <p className="note">
              <Icon name="info" size={13} />
              {p.otherBranch(c.otherBranch)}
            </p>
          )}
          {c?.fetchError && (
            <p className="note note--warn">
              <Icon name="alert" size={13} />
              {p.fetchError(c.fetchError)}
            </p>
          )}
          {error && <p className="note note--error">{error}</p>}
          <small className="faint flow-fine">{p.readOnly}</small>
        </div>

        <footer className="sheet-foot flow-foot">
          <span className="flow-actions">
            {state.kind === "done" ? (
              <button className="btn btn--primary btn--sm" autoFocus onClick={onClose}>
                {p.close}
              </button>
            ) : (
              <>
                <button className="btn btn--ghost btn--sm" disabled={state.kind === "finishing"} onClick={onClose}>
                  {p.cancel}
                </button>
                {!pushed && (
                  <button className="btn btn--sm" disabled={busy} onClick={check}>
                    <Icon name="refresh" size={12} />
                    {p.recheck}
                  </button>
                )}
                <button
                  className="btn btn--primary btn--sm"
                  disabled={busy}
                  onClick={() => finish(c, !pushed)}
                >
                  <Icon name="check" size={12} />
                  {state.kind === "finishing" ? p.finishing : pushed ? p.finish : p.finishAnyway}
                </button>
              </>
            )}
          </span>
        </footer>
      </div>
    </div>
  );
}
