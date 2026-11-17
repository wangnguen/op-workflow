import { gateLabel, stepLabel, vi } from "../../i18n/vi";
import type { AgentKind, AgentView, OrbitSnapshot, SessionStatus, StepKind, TaskSystem } from "../../ipc/types";
import { STEP_COUNT } from "../../shared/steps";

export interface SessionItem {
  id: string;
  /** the AI of the session; absent for a task in "Đang làm" that has no AI session yet */
  agent?: AgentKind;
  /** task the item belongs to (in-app sessions and tasks) */
  taskId?: string;
  title: string;
  detail: string;
  /** branch of the task, for in-app items */
  branch?: string;
  /** repo of the task, for in-app items */
  repo?: string;
  /** OpenProject status of the task, for in-app items */
  nxStatus?: string;
  nxStatusColor?: string;
  status: SessionStatus;
  /** 0..100 of the handoff threshold, when known */
  contextPct?: number;
  inApp: boolean;
}

export type SessionGroups = Record<SessionStatus, SessionItem[]>;

function flatten(agents: AgentView[]): AgentView[] {
  return agents.flatMap((a) => [a, ...flatten(a.children)]);
}

/** What every in-app card shows about its task: name, OpenProject status, repo, branch. */
function taskInfo(a: TaskSystem) {
  return {
    taskId: a.taskId,
    title: `${a.taskId} · ${a.title}`,
    branch: a.branch || undefined,
    repo: a.repo || undefined,
    nxStatus: a.nxStatus || undefined,
    nxStatusColor: a.nxStatusColor,
    inApp: true,
  };
}

/** "3/8 Viết code" */
function stepText(a: TaskSystem, kind: StepKind): string {
  const s = a.steps.find((x) => x.kind === kind);
  return s ? `${s.index}/${STEP_COUNT} ${stepLabel[kind]}` : stepLabel[kind];
}

/** A task with no AI running: the last finished step, and it is the user's turn. */
function taskItem(a: TaskSystem): SessionItem {
  const last = [...a.steps].reverse().find((s) => s.state === "done");
  return {
    ...taskInfo(a),
    id: `task:${a.taskId}`,
    detail: last ? vi.sessions.yourTurn(stepLabel[last.kind]) : vi.sessions.notStarted,
    status: "waiting",
  };
}

/** Group every known session (app agents, tasks waiting for you, outside sessions) by status. */
export function groupSessions(s: OrbitSnapshot): SessionGroups {
  const groups: SessionGroups = { waiting: [], running: [], idle: [] };
  for (const a of [s.active, ...(s.others ?? [])]) {
    if (!a) continue;
    if (a.agents.length === 0) {
      groups.waiting.push(taskItem(a));
      continue;
    }
    for (const agent of flatten(a.agents)) {
      const main = agent === a.agents[0];
      const status: SessionStatus = a.gate && main ? "waiting" : agent.status;
      const step = stepText(a, agent.step);
      const detail =
        a.gate && main
          ? `${step} · ${gateLabel[a.gate.kind]}`
          : main && agent.status === "idle"
            ? vi.flow.ready(step)
            : step;
      groups[status].push({
        ...taskInfo(a),
        id: agent.id,
        agent: agent.kind,
        detail,
        status,
        contextPct: agent.handoffAt > 0 ? (agent.contextTokens / agent.handoffAt) * 100 : undefined,
      });
    }
  }
  for (const d of s.distant) {
    groups[d.status].push({
      id: d.id,
      agent: d.agent,
      title: d.label,
      detail: vi.sessions.outside,
      status: d.status,
      inApp: false,
    });
  }
  return groups;
}
