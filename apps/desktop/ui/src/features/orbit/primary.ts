import { stepLabel, vi } from "../../i18n/vi";
import type { OrbitSnapshot, StepKind } from "../../ipc/types";
import { gateTab, type TaskTab } from "../task/taskTabs";
import { findAgent, type Selection } from "./selection";

/** Where a map item leads. */
export type Go =
  /** the task's session in Workspace, at the message a step started with or at its question */
  | { to: "session"; taskId: string; step?: StepKind; ask?: boolean }
  /** a Workspace session by id (one without a task) */
  | { to: "chat"; sessionId: string; ask?: boolean }
  /** a session opened outside the app, read in Workspace */
  | { to: "outside"; id: string }
  | { to: "task"; taskId: string; tab?: TaskTab }
  | { to: "start"; taskId: string };

/** The detail panel's main button (a double click on the item does the same). */
export interface Primary {
  label: string;
  hint: string;
  go: Go;
}

/** Steps the app sends into the task's session as messages (labelled with the step there). */
const SESSION_STEPS: readonly StepKind[] = ["analyze", "reviewLocal", "fix", "fixCross", "pushSummary"];

/** Whether a step runs in the task's session: the ones above, and Viết code when Claude writes
 *  it (Codex writes it in its own process). The others (Review chéo, Harness, Test trình
 *  duyệt, Review chốt…) run on their own and show in the task window's Chat tab. */
export function runsInSession(step: StepKind, model?: string): boolean {
  if (step === "implement") return model === undefined || !/gpt|codex|^o\d/i.test(model);
  return SESSION_STEPS.includes(step);
}

const t = vi.orbit.panel;

/** What the main button of the picked item does. `waiting` = task ids whose session waits for an
 *  answer (a permission, a question, a plan). */
export function primaryAction(sel: Selection | null, s: OrbitSnapshot, waiting: ReadonlySet<string>): Primary | undefined {
  if (!sel) return undefined;
  const a = s.active;
  const session = (taskId: string): Primary => ({ label: t.goSession, hint: t.goSessionHint, go: { to: "session", taskId } });
  const step = (taskId: string, kind: StepKind): Primary => {
    const model = a?.steps.find((x) => x.kind === kind)?.model;
    return runsInSession(kind, model)
      ? { label: t.goStep, hint: t.goStepHint(stepLabel[kind]), go: { to: "session", taskId, step: kind } }
      : { label: t.stepChat, hint: t.stepChatHint, go: { to: "task", taskId, tab: "chat" } };
  };
  switch (sel.kind) {
    case "star":
      return a && session(a.taskId);
    case "agent": {
      const agent = a && findAgent(a.agents, sel.id);
      return a && agent && step(a.taskId, agent.step);
    }
    case "step": {
      const st = a?.steps.find((x) => x.kind === sel.step);
      if (!a || !st) return undefined;
      // nothing ran yet: the session itself
      return st.state === "pending" || st.state === "skipped" ? session(a.taskId) : step(a.taskId, st.kind);
    }
    case "gate":
      if (!a?.gate) return undefined;
      return waiting.has(a.taskId)
        ? { label: t.goAsk, hint: t.goAskHint, go: { to: "session", taskId: a.taskId, ask: true } }
        : { label: t.openGate, hint: t.openGateHint, go: { to: "task", taskId: a.taskId, tab: gateTab(a.gate.kind) } };
    case "finding":
      return a && { label: vi.flow.tabs.findings, hint: t.findingsHint, go: { to: "task", taskId: a.taskId, tab: "findings" } };
    case "nebula":
      return { label: vi.board.start, hint: t.startHint, go: { to: "start", taskId: sel.taskId } };
    case "distant":
      // the app's own session without a task opens as usual; one opened outside is read only
      return s.distant.find((d) => d.id === sel.id)?.own
        ? { label: t.goSession, hint: t.goOwnHint, go: { to: "chat", sessionId: sel.id } }
        : { label: t.goOutside, hint: t.goOutsideHint, go: { to: "outside", id: sel.id } };
  }
}
