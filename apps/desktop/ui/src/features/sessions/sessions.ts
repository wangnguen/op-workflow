import { stepLabel, vi } from "../../i18n/vi";
import type { AgentKind, AgentView, OrbitSnapshot, SessionStatus, TaskSystem } from "../../ipc/types";

export interface SessionItem {
  id: string;
  /** the AI of the session; absent for a task in "Đang làm" that has no AI session yet */
  agent?: AgentKind;
  /** task the item belongs to (in-app sessions and tasks) */
  taskId?: string;
  title: string;
  detail: string;
  /** branch of the task, for task items */
  branch?: string;
  status: SessionStatus;
  /** 0..100 of the handoff threshold, when known */
  contextPct?: number;
  inApp: boolean;
}

export type SessionGroups = Record<SessionStatus, SessionItem[]>;

function flatten(agents: AgentView[]): AgentView[] {
  return agents.flatMap((a) => [a, ...flatten(a.children)]);
}

/** A task with no AI running: the last finished step, and it is the user's turn. */
function taskItem(a: TaskSystem): SessionItem {
  const last = [...a.steps].reverse().find((s) => s.state === "done");
  return {
    id: `task:${a.taskId}`,
    taskId: a.taskId,
    title: `${a.taskId} · ${a.title}`,
    detail: last ? vi.sessions.yourTurn(stepLabel[last.kind]) : vi.sessions.notStarted,
    branch: a.branch || undefined,
    status: "waiting",
    inApp: true,
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
      const status: SessionStatus = a.gate && agent === a.agents[0] ? "waiting" : agent.status;
      groups[status].push({
        id: agent.id,
        agent: agent.kind,
        taskId: a.taskId,
        title: `${a.taskId} · ${stepLabel[agent.step]}`,
        detail: a.repo,
        status,
        contextPct: agent.handoffAt > 0 ? (agent.contextTokens / agent.handoffAt) * 100 : undefined,
        inApp: true,
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
