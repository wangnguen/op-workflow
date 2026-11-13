import { stepLabel, vi } from "../../i18n/vi";
import type { AgentKind, AgentView, OrbitSnapshot, SessionStatus } from "../../ipc/types";

export interface SessionItem {
  id: string;
  agent: AgentKind;
  title: string;
  detail: string;
  status: SessionStatus;
  /** 0..100 of the handoff threshold, when known */
  contextPct?: number;
  inApp: boolean;
}

export type SessionGroups = Record<SessionStatus, SessionItem[]>;

function flatten(agents: AgentView[]): AgentView[] {
  return agents.flatMap((a) => [a, ...flatten(a.children)]);
}

/** Group every known session (app agents + outside sessions) by status. */
export function groupSessions(s: OrbitSnapshot): SessionGroups {
  const groups: SessionGroups = { waiting: [], running: [], idle: [] };
  for (const a of [s.active, ...(s.others ?? [])]) {
    if (!a) continue;
    for (const agent of flatten(a.agents)) {
      const status: SessionStatus = a.gate && agent === a.agents[0] ? "waiting" : agent.status;
      groups[status].push({
        id: agent.id,
        agent: agent.kind,
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
