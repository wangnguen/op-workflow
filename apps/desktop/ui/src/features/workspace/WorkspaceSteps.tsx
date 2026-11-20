import { useEffect, useRef, useState } from "react";
import { stepLabel, vi } from "../../i18n/vi";
import {
  completeWorkspaceStep, errorMessage, getWorkspaceFlow, workspaceFlowAction,
} from "../../ipc/client";
import type { FlowAction, SessionView, StepKind, WorkspaceFlow } from "../../ipc/types";
import { StatusIcon } from "../../shared/ui/StatusIcon";
import { StartOver } from "../task/StartOver";

const t = vi.workspace.steps;
const POLL_MS = 1200;

/** The task's steps above its chat (PLAN.md §12 C): turn the app's steps on, pick which ones,
 *  run one as a message in the session; a step ends when Claude reports it or you press Xong. */
export function WorkspaceSteps({
  wpId, session, beforeRun, onChanged, onOpenTask, onSession,
}: {
  wpId: number;
  session: SessionView | undefined;
  /** makes sure the task has its session before a step is sent into it */
  beforeRun: () => Promise<void>;
  onChanged: () => void;
  onOpenTask: (wpId: number, tab: string) => void;
  /** the task's steps go to another session now (after Làm lại từ đầu): show it */
  onSession: (id: string) => void;
}) {
  const [flow, setFlow] = useState<WorkspaceFlow>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  // why a step that cannot run yet was not run (shown where the error goes)
  const [notice, setNotice] = useState<string>();
  const [edit, setEdit] = useState(false);
  // read by the poll without restarting it
  const acting = useRef(false);

  useEffect(() => {
    let alive = true;
    let reading = false;
    const read = async () => {
      if (reading || acting.current) return;
      reading = true;
      try {
        const next = await getWorkspaceFlow(wpId);
        if (alive && !acting.current) setFlow(next);
      } catch (e) {
        if (alive) setError(errorMessage(e));
      } finally {
        reading = false;
      }
    };
    void read();
    const timer = window.setInterval(() => void read(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [wpId]);

  const run = async (work: () => Promise<void>) => {
    acting.current = true;
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      await work();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      acting.current = false;
      setBusy(false);
    }
  };
  const act = (action: FlowAction) =>
    run(async () => {
      if (action.type === "runWorkspaceStep") await beforeRun();
      setFlow(await workspaceFlowAction(wpId, action));
      onChanged();
    });
  const done = (step: StepKind) => run(() => completeWorkspaceStep(wpId, step));
  // after Làm lại từ đầu the task's steps go to its new session: show that one
  const startedOver = () =>
    run(async () => {
      const next = await getWorkspaceFlow(wpId);
      setFlow(next);
      if (next.session && next.session !== session?.id) onSession(next.session);
      onChanged();
    });

  if (!flow) return error ? <p className="note note--error ws-flow">{error}</p> : null;
  const disabled = busy || !!flow.running;
  // the session cannot take a step now: a turn runs, it waits for you, or it is open elsewhere
  const occupied = session?.terminal || session?.state === "outside" ||
    session?.state === "running" || session?.state === "waiting";
  const gateTab = flow.gate === "findings" || flow.gate === "afterFix" ? "findings"
    : flow.gate === "planReview" ? "plan" : "progress";
  const message = error ?? flow.error?.message;

  return (
    <section className="ws-flow" aria-label={t.label}>
      <div className="ws-flow-controls">
        <label className="ws-flow-option">
          <span className="ws-flow-check">
            <input type="checkbox" checked={flow.enabled} disabled={disabled}
              onChange={(e) => void act({ type: "setWorkspaceFlow", on: e.target.checked })} />
            {t.flow}
          </span>
          <small className="faint">{t.flowAbout}</small>
        </label>
        {flow.enabled && (
          <label className="ws-flow-option">
            <span className="ws-flow-check">
              <input type="checkbox" checked={flow.auto} disabled={busy}
                onChange={(e) => void act({ type: "setAuto", on: e.target.checked })} />
              {t.auto}
            </span>
            <small className="faint">{t.autoAbout}</small>
          </label>
        )}
        {flow.enabled && (
          <div className="ws-flow-buttons">
            <button className="btn btn--sm" aria-pressed={edit} title={t.pickAbout} onClick={() => setEdit(!edit)}>
              {t.pick}
            </button>
            <button className="btn btn--sm btn--ghost" title={t.openAbout} onClick={() => onOpenTask(wpId, gateTab)}>
              {flow.gate ? t.openGate : t.openTask}
            </button>
            <StartOver wpId={wpId} small disabled={disabled} onDone={() => void startedOver()} />
          </div>
        )}
      </div>
      {flow.enabled && (
        <div className="ws-flow-steps">
          {flow.steps.filter((s) => edit || s.enabled).map((s) => edit ? (
            <label className="ws-flow-choice" key={s.step}>
              <input type="checkbox" checked={s.enabled} disabled={disabled}
                onChange={(e) => void act({ type: "setWorkspaceStep", step: s.step, on: e.target.checked })} />
              {stepLabel[s.step]}
            </label>
          ) : (
            // a step that cannot run yet stays clickable and says why instead of running
            <button key={s.step} className={`btn btn--sm${s.whyNot ? " is-blocked" : ""}`}
              aria-disabled={s.whyNot ? true : undefined}
              disabled={disabled || !!occupied}
              title={s.whyNot ?? t.run(stepLabel[s.step])}
              onClick={() => s.whyNot
                ? setNotice(`${t.notReady(stepLabel[s.step])}. ${s.whyNot}`)
                : void act({ type: "runWorkspaceStep", step: s.step })}>
              <StatusIcon state={s.state === "running" && flow.running !== s.step ? "pending" : s.state} />
              {stepLabel[s.step]}
            </button>
          ))}
        </div>
      )}
      {edit && flow.enabled && <p className="faint ws-flow-hint">{t.pickAbout}</p>}
      {flow.running && (
        <div className="ws-flow-running">
          <span className="dim">{t.running(stepLabel[flow.running])}</span>
          {flow.sessionStep && (
            <button className="btn btn--sm" title={t.doneAbout} disabled={busy || !!occupied}
              onClick={() => flow.running && void done(flow.running)}>
              {t.done}
            </button>
          )}
          <button className="btn btn--sm btn--ghost" title={t.stopAbout} disabled={busy}
            onClick={() => void act({ type: "stop" })}>
            {t.stop}
          </button>
        </div>
      )}
      {message && <p className="note note--error">{message}</p>}
      {notice && !message && <p className="note note--warn">{notice}</p>}
    </section>
  );
}
