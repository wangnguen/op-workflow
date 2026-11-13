import type { AgentView, OrbitSnapshot, StepKind } from "../../ipc/types";

/** What the detail panel shows. */
export type Selection =
  | { kind: "star" }
  | { kind: "step"; step: StepKind }
  | { kind: "agent"; id: string }
  | { kind: "finding"; fingerprint: string }
  | { kind: "gate" }
  | { kind: "nebula"; taskId: string }
  | { kind: "distant"; id: string };

export function findAgent(agents: AgentView[], id: string): AgentView | undefined {
  for (const a of agents) {
    if (a.id === id) return a;
    const child = findAgent(a.children, id);
    if (child) return child;
  }
  return undefined;
}

/** Default focus: the gate if one is waiting, else the agent, else the star. */
export function autoSelection(s: OrbitSnapshot | undefined): Selection | null {
  if (!s) return null;
  const a = s.active;
  if (!a) {
    const first = s.inbox[0];
    return first ? { kind: "nebula", taskId: first.taskId } : null;
  }
  if (a.gate) return { kind: "gate" };
  const agent = a.agents[0];
  return agent ? { kind: "agent", id: agent.id } : { kind: "star" };
}

/** Whether the selected thing still exists in the snapshot. */
export function isValidSelection(sel: Selection, s: OrbitSnapshot): boolean {
  const a = s.active;
  switch (sel.kind) {
    case "star":
    case "step":
      return a !== undefined;
    case "agent":
      return a !== undefined && findAgent(a.agents, sel.id) !== undefined;
    case "finding":
      return a !== undefined && a.findings.some((f) => f.fingerprint === sel.fingerprint);
    case "gate":
      return a?.gate !== undefined;
    case "nebula":
      return a === undefined && s.inbox.some((i) => i.taskId === sel.taskId);
    case "distant":
      return s.distant.some((d) => d.id === sel.id);
  }
}

export function resolveSelection(sel: Selection | null, s: OrbitSnapshot | undefined): Selection | null {
  if (!s) return null;
  if (sel && isValidSelection(sel, s)) return sel;
  return autoSelection(s);
}

export function sameSelection(a: Selection | null, b: Selection | null): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}
