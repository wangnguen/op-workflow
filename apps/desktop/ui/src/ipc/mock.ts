// Mock data for browser mode, with a looping demo timeline so the UI animates.
// Every DEMO_FRAME_MS the active task advances one frame through the 8 steps.
import type {
  AgentKind,
  AgentView,
  AppInfo,
  BoardSnapshot,
  DistantSession,
  FindingState,
  FindingView,
  GateKind,
  InboxItem,
  OrbitSnapshot,
  QuotaSnapshot,
  SettingsView,
  SessionStatus,
  StepKind,
  StepState,
  StepView,
  SubagentView,
  TaskCard,
  TaskSystem,
} from "./types";
import { STEP_COUNT, STEP_ORDER } from "../shared/steps";

export const DEMO_FRAME_MS = 4000;
/** One demo frame stands for this much "real" time in timestamps. */
const FRAME_TIME_MS = 150_000;
const T0 = Date.now();

export { STEP_ORDER };

const FIRST_FRAME: Record<StepKind, number> = {
  intake: 0,
  analyze: 1,
  implement: 3,
  reviewLocal: 7,
  fix: 8,
  crossReview: 9,
  finalReview: 13,
  pushSummary: 15,
};

const ACTIVE_ID = "WP-187716";

const FINDINGS = {
  f1: { severity: "high", title: "Null khi user chưa có org", file: "api/src/routes/files.ts", line: 42, source: "codex" },
  f2: { severity: "medium", title: "Thiếu trạng thái loading khi tải danh sách", file: "web/src/pages/files.tsx", line: 88, source: "codex" },
  f3: { severity: "medium", title: "Chưa có test cho user không có org", file: "api/test/files.test.ts", source: "codex" },
  f4: { severity: "low", title: "Tên biến khó hiểu", file: "api/src/lib/util.ts", line: 10, source: "codex" },
  f5: { severity: "low", title: "Comment thừa sau khi sửa", file: "api/src/routes/files.ts", line: 57, source: "review-local" },
  f6: { severity: "high", title: "Chưa kiểm tra quyền khi đọc thư mục chia sẻ", file: "api/src/services/box.ts", line: 133, source: "review-local" },
  f7: { severity: "medium", title: "Thiếu log khi Box API trả về 429", file: "api/src/services/box.ts", line: 210, source: "review-local" },
} as const satisfies Record<string, Omit<FindingView, "fingerprint" | "state">>;

type FindingKey = keyof typeof FINDINGS;

interface AgentSpec {
  kind: AgentKind;
  status: SessionStatus;
  ctxPct: number;
  compactions: number;
  subagents: SubagentView[];
  child?: SessionStatus;
  tools: string[];
}

interface Frame {
  active: boolean;
  step: number;
  state: StepState;
  overrides?: Partial<Record<StepKind, StepState>>;
  round?: number;
  /** times fix has run: after the first review-local, then after each Codex round */
  fixRound?: number;
  gate?: { kind: GateKind; message: string };
  agent?: AgentSpec;
  findings: Partial<Record<FindingKey, FindingState>>;
  newComments: number;
  cost: number;
}

const sub = (name: string, status: SubagentView["status"] = "running"): SubagentView => ({ name, status });

const ALL_OPEN = { f1: "open", f2: "open", f3: "open", f4: "open" } as const;
const AFTER_R2 = { f1: "fixed", f2: "fixed", f3: "accepted", f4: "deferred" } as const;

export const DEMO_FRAMES: readonly Frame[] = [
  { active: true, step: 0, state: "running", findings: {}, newComments: 0, cost: 0 },
  {
    active: true, step: 1, state: "running", findings: {}, newComments: 0, cost: 0.05,
    agent: { kind: "claude", status: "running", ctxPct: 8, compactions: 0, subagents: [sub("Explore")], tools: ["Grep", "Read", "Glob"] },
  },
  {
    active: true, step: 1, state: "waitingUser", findings: {}, newComments: 0, cost: 0.09,
    gate: { kind: "planReview", message: "plan.md đã sẵn sàng: 2 phương án, đề xuất phương án A" },
    agent: { kind: "claude", status: "waiting", ctxPct: 14, compactions: 0, subagents: [sub("Explore", "done")], tools: ["Write", "Read", "Grep"] },
  },
  {
    active: true, step: 2, state: "running", findings: {}, newComments: 0, cost: 0.18,
    agent: { kind: "claude", status: "running", ctxPct: 31, compactions: 0, subagents: [sub("Explore")], tools: ["Edit", "Read", "Grep"] },
  },
  {
    active: true, step: 2, state: "running", findings: {}, newComments: 1, cost: 0.31,
    agent: { kind: "claude", status: "running", ctxPct: 58, compactions: 0, subagents: [sub("Explore", "done"), sub("general-purpose"), sub("Plan")], tools: ["Edit", "Bash", "Edit"] },
  },
  {
    active: true, step: 2, state: "running", findings: {}, newComments: 1, cost: 0.42,
    agent: { kind: "claude", status: "running", ctxPct: 96, compactions: 0, subagents: [sub("general-purpose")], tools: ["Bash", "Edit", "Read"] },
  },
  {
    active: true, step: 2, state: "running", findings: {}, newComments: 1, cost: 0.47,
    agent: { kind: "claude", status: "running", ctxPct: 22, compactions: 1, subagents: [], tools: ["Read", "Edit"] },
  },
  // first /review-local, then Claude fixes what it found before Codex looks
  {
    active: true, step: 3, state: "running", findings: {}, newComments: 0, cost: 0.53,
    agent: { kind: "claude", status: "running", ctxPct: 24, compactions: 1, subagents: [], tools: ["Bash", "Read", "Grep"] },
  },
  {
    active: true, step: 4, state: "running", fixRound: 1, findings: { f6: "fixing" }, newComments: 0, cost: 0.58,
    agent: { kind: "claude", status: "running", ctxPct: 27, compactions: 1, subagents: [], tools: ["Edit", "Bash"] },
  },
  // Codex ⇄ Claude until Codex is clean
  {
    active: true, step: 5, state: "running", round: 1, fixRound: 1, findings: { f6: "fixed" }, newComments: 0, cost: 0.62,
    agent: { kind: "claude", status: "idle", ctxPct: 27, compactions: 1, subagents: [], child: "running", tools: ["Bash"] },
  },
  {
    active: true, step: 5, state: "running", round: 1, fixRound: 1, findings: { f6: "fixed", ...ALL_OPEN }, newComments: 0, cost: 0.67,
    agent: { kind: "claude", status: "idle", ctxPct: 27, compactions: 1, subagents: [], child: "running", tools: ["Bash"] },
  },
  {
    active: true, step: 4, state: "running", round: 1, fixRound: 2, overrides: { crossReview: "done" },
    findings: { f6: "fixed", ...ALL_OPEN, f1: "fixing", f2: "fixing" }, newComments: 0, cost: 0.74,
    agent: { kind: "claude", status: "running", ctxPct: 35, compactions: 1, subagents: [], tools: ["Edit", "Edit", "Bash"] },
  },
  {
    active: true, step: 5, state: "running", round: 2, fixRound: 2, findings: { f6: "fixed", ...AFTER_R2 }, newComments: 0, cost: 0.81,
    agent: { kind: "claude", status: "idle", ctxPct: 37, compactions: 1, subagents: [], child: "running", tools: ["Bash"] },
  },
  // final /review-local, gate G2 for the medium finding, then push-summary
  {
    active: true, step: 6, state: "running", round: 2, fixRound: 2, findings: { f6: "fixed", ...AFTER_R2, f5: "open" }, newComments: 0, cost: 0.88,
    agent: { kind: "claude", status: "running", ctxPct: 41, compactions: 1, subagents: [], tools: ["Bash", "Read"] },
  },
  {
    active: true, step: 6, state: "waitingUser", round: 2, fixRound: 2, findings: { f6: "fixed", ...AFTER_R2, f5: "open", f7: "open" }, newComments: 0, cost: 0.92,
    gate: { kind: "mediumTriage", message: "/review-local chốt báo 1 finding medium: sửa, chấp nhận hay để sau?" },
    agent: { kind: "claude", status: "waiting", ctxPct: 43, compactions: 1, subagents: [], tools: ["Bash"] },
  },
  {
    active: true, step: 7, state: "running", round: 2, fixRound: 2, findings: { f6: "fixed", ...AFTER_R2, f5: "deferred", f7: "accepted" }, newComments: 0, cost: 0.98,
    agent: { kind: "claude", status: "running", ctxPct: 45, compactions: 1, subagents: [], tools: ["Bash", "Bash"] },
  },
  { active: false, step: 7, state: "done", findings: {}, newComments: 0, cost: 1.04 },
  { active: false, step: 7, state: "done", findings: {}, newComments: 0, cost: 1.04 },
];

/** `?frame=N` in the browser URL pins the demo to one frame (for reviews and screenshots). */
function pinnedFrame(): number | undefined {
  if (typeof window === "undefined") return undefined;
  const raw = new URLSearchParams(window.location.search).get("frame");
  if (raw === null) return undefined;
  const n = Number.parseInt(raw, 10);
  return Number.isInteger(n) && n >= 0 && n < DEMO_FRAMES.length ? n : undefined;
}

export function demoFrameIndex(now: number): number {
  const pinned = pinnedFrame();
  if (pinned !== undefined) return pinned;
  const n = DEMO_FRAMES.length;
  return ((Math.floor((now - T0) / DEMO_FRAME_MS) % n) + n) % n;
}

function frameAt(index: number): Frame {
  const f = DEMO_FRAMES[index % DEMO_FRAMES.length];
  if (!f) throw new Error(`demo frame ${index} missing`);
  return f;
}

const iso = (ms: number) => new Date(ms).toISOString();

// ---------- static mock data ----------

const INBOX: InboxItem[] = [
  { taskId: "WP-188001", title: "Lỗi đồng bộ quyền khi đổi owner thư mục", project: "Web Platform" },
  { taskId: "WP-188002", title: "Thêm bộ lọc ngày cho báo cáo chia sẻ", project: "Mobile App" },
];

const DISTANT: DistantSession[] = [
  { id: "ext-1", agent: "claude", label: "acme-desktop", status: "idle" },
  { id: "ext-2", agent: "codex", label: "misfile-buster", status: "running" },
  { id: "ext-3", agent: "claude", label: "study-workspace", status: "waiting" },
];

export function mockAppInfo(): AppInfo {
  return { version: "0.1.0-demo", core: "mock" };
}

export function mockQuota(frameIndex: number, now: number): QuotaSnapshot[] {
  const hour = 3_600_000;
  return [
    {
      provider: "claude",
      plan: "team",
      source: "rate_limit_event",
      observedAt: iso(now - 20_000),
      windows: [
        { kind: "fiveHour", usedPct: Math.min(100, 17 + frameIndex * 0.6), resetsAt: iso(now + 2 * hour + 14 * 60_000) },
        { kind: "sevenDay", usedPct: 74 + frameIndex * 0.1, resetsAt: iso(now + 4 * 24 * hour) },
      ],
    },
    {
      provider: "codex",
      plan: "plus",
      source: "app-server",
      observedAt: iso(now - 8_000),
      windows: [
        { kind: "fiveHour", usedPct: frameIndex >= 9 ? 3 + (frameIndex - 8) * 0.8 : 3, resetsAt: iso(now + 3 * hour + 5 * 60_000) },
        { kind: "sevenDay", usedPct: frameIndex >= 9 ? 0.5 : 0, resetsAt: iso(now + 7 * 24 * hour - hour) },
      ],
    },
  ];
}

// ---------- builders ----------

function buildSteps(f: Frame, frameIndex: number, now: number): StepView[] {
  const taskStart = now - frameIndex * FRAME_TIME_MS;
  return STEP_ORDER.map((kind, i) => {
    let state: StepState = i < f.step ? "done" : i === f.step ? f.state : "pending";
    const override = f.overrides?.[kind];
    if (override) state = override;
    const view: StepView = { kind, index: i + 1, state };
    if (state !== "pending") {
      const started = taskStart + FIRST_FRAME[kind] * FRAME_TIME_MS;
      view.startedAt = iso(started);
      if (state === "done") view.endedAt = iso(started + FRAME_TIME_MS * 0.9);
      view.costUsd = Math.round((0.04 + i * 0.03) * 100) / 100;
    }
    // Fix runs after the first review-local and after Codex round 1; Codex runs two rounds.
    if (kind === "crossReview" && state !== "pending") view.round = f.round ?? 1;
    if (kind === "fix" && state !== "pending") view.round = f.fixRound ?? 1;
    return view;
  });
}

function buildAgents(f: Frame, now: number): AgentView[] {
  const spec = f.agent;
  if (!spec) return [];
  const step = STEP_ORDER[f.step] ?? "intake";
  const handoffAt = 500_000;
  const tools = spec.tools.map((name, i) => ({ at: iso(now - (i * 37 + 6) * 1000), name }));
  const children: AgentView[] = spec.child
    ? [
        {
          id: "codex-review",
          kind: "codex",
          status: spec.child,
          step: "crossReview",
          contextTokens: 64_000,
          contextWindow: 500_000,
          handoffAt: 400_000,
          compactions: 0,
          subagents: [],
          children: [],
          recentTools: [{ at: iso(now - 4000), name: "shell: git diff" }],
        },
      ]
    : [];
  return [
    {
      id: "claude-main",
      kind: spec.kind,
      status: spec.status,
      step,
      contextTokens: Math.round((spec.ctxPct / 100) * handoffAt),
      contextWindow: 1_000_000,
      handoffAt,
      compactions: spec.compactions,
      subagents: spec.subagents,
      children,
      recentTools: tools,
    },
  ];
}

function buildFindings(f: Frame): FindingView[] {
  return (Object.keys(f.findings) as FindingKey[]).flatMap((key) => {
    const state = f.findings[key];
    return state ? [{ ...FINDINGS[key], fingerprint: key, state }] : [];
  });
}

export function orbitForFrame(frameIndex: number, now: number): OrbitSnapshot {
  const f = frameAt(frameIndex);
  const quota = mockQuota(frameIndex, now);
  if (!f.active) return { inbox: INBOX, distant: DISTANT, quota };
  const active: TaskSystem = {
    taskId: ACTIVE_ID,
    title: "Không tải được danh sách file khi user chưa có org",
    repo: "acme-web",
    branch: "feature/187716",
    nxStatus: "In progress",
    startedAt: iso(now - frameIndex * FRAME_TIME_MS),
    steps: buildSteps(f, frameIndex, now),
    agents: buildAgents(f, now),
    findings: buildFindings(f),
    newComments: f.newComments,
  };
  if (f.gate) active.gate = { kind: f.gate.kind, since: iso(now - 40_000), message: f.gate.message };
  return { active, inbox: INBOX, distant: DISTANT, quota };
}

export function boardForFrame(frameIndex: number, now: number): BoardSnapshot {
  const f = frameAt(frameIndex);
  const runningAgent: AgentKind | undefined = f.agent ? (f.agent.child === "running" ? "codex" : f.agent.kind) : undefined;
  const activeCard: TaskCard = {
    id: ACTIVE_ID,
    wpId: 187716,
    title: "Không tải được danh sách file khi user chưa có org",
    project: "Web Platform",
    repo: "acme-web",
    branch: "feature/187716",
    nxStatus: "In progress",
    column: f.active ? "active" : "awaitingPush",
    step: {
      kind: STEP_ORDER[f.step] ?? "intake",
      index: f.step + 1,
      state: f.state,
      since: iso(now - 60_000),
    },
    costUsd: f.cost,
    newComments: f.newComments,
  };
  if (runningAgent) activeCard.agent = runningAgent;
  if (f.agent) activeCard.contextPct = f.agent.ctxPct;

  const tasks: TaskCard[] = [
    { id: "WP-188001", wpId: 188001, title: INBOX[0]?.title ?? "", project: "Web Platform", nxStatus: "Open", column: "inbox", newComments: 2 },
    { id: "WP-188002", wpId: 188002, title: INBOX[1]?.title ?? "", project: "Mobile App", nxStatus: "Reopened", column: "inbox", newComments: 0 },
    activeCard,
    {
      id: "WP-186196",
      wpId: 186196,
      title: "Sửa thông báo lỗi khi token Box hết hạn",
      project: "Web Platform",
      repo: "acme-api",
      branch: "feature/186196",
      nxStatus: "In progress",
      column: "awaitingPush",
      step: { kind: "pushSummary", index: STEP_COUNT, state: "done", since: iso(now - 50 * 60_000) },
      agent: "claude",
      costUsd: 1.37,
      newComments: 0,
    },
  ];
  const board: BoardSnapshot = {
    tasks,
    quota: mockQuota(frameIndex, now),
    today: { tokens: 1_240_000 + frameIndex * 18_000, costUsd: 0.84 + f.cost },
    mode: "demo",
  };
  if (f.active) board.activeTaskId = ACTIVE_ID;
  return board;
}

export const mockOrbit = (now: number): OrbitSnapshot => orbitForFrame(demoFrameIndex(now), now);

// ---------- settings (browser preview only; nothing is stored) ----------

let mockTokenSet = false;
let mockBaseUrl = "";

export function mockSettings(): SettingsView {
  return { nxBaseUrl: mockBaseUrl, nxTokenSet: mockTokenSet };
}

export function mockSaveToken(baseUrl: string, token: string): SettingsView {
  if (!baseUrl.trim().startsWith("https://")) throw { code: "url.invalid", message: "URL phải bắt đầu bằng https://" };
  if (!token.trim()) throw { code: "token.empty", message: "Token trống" };
  mockBaseUrl = baseUrl.trim().replace(/\/+$/, "");
  mockTokenSet = true;
  return { ...mockSettings(), nxUser: "Demo User", nxTaskCount: 0 };
}

export function mockClearToken(): SettingsView {
  mockTokenSet = false;
  return mockSettings();
}
export const mockBoard = (now: number): BoardSnapshot => boardForFrame(demoFrameIndex(now), now);
