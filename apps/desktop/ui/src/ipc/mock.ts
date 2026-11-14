// Mock data for browser mode, with a looping demo timeline so the UI animates.
// Every DEMO_FRAME_MS the active task advances one frame through the 8 steps.
import type {
  AgentKind,
  AgentView,
  AppInfo,
  BoardSnapshot,
  DeleteBranchRequest,
  DistantSession,
  FindingState,
  FindingView,
  GateKind,
  InboxItem,
  IssueFetch,
  LinkStatus,
  OrbitSnapshot,
  Provider,
  ProviderLink,
  QuotaSnapshot,
  RepoState,
  SettingsView,
  SessionStatus,
  StartOptions,
  StartRequest,
  StartResult,
  StepKind,
  StepState,
  StepView,
  SubagentView,
  TaskCard,
  TaskSystem,
  TasksDirChange,
  UpdateView,
} from "./types";
import { STEP_COUNT, STEP_ORDER } from "../shared/steps";
import { mockFlowCard, mockFlowSystem, mockRegisterFlow } from "./mockFlow";

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

const BASE_DISTANT: DistantSession[] = [
  { id: "ext-1", agent: "claude", label: "acme-desktop", status: "idle" },
  { id: "ext-2", agent: "codex", label: "misfile-buster", status: "running" },
  { id: "ext-3", agent: "claude", label: "study-workspace", status: "waiting" },
];

/** `?inbox=N&distant=N` in the browser URL fills the map with N items (layout reviews). */
function urlCount(key: string): number | undefined {
  if (typeof window === "undefined") return undefined;
  const raw = new URLSearchParams(window.location.search).get(key);
  const n = raw === null ? Number.NaN : Number.parseInt(raw, 10);
  return Number.isInteger(n) && n >= 0 && n <= 60 ? n : undefined;
}

const STATUSES: SessionStatus[] = ["idle", "running", "waiting"];

const INBOX_VIEW: InboxItem[] = (() => {
  const n = urlCount("inbox");
  if (n === undefined) return INBOX;
  // every third one has a long Japanese title, as many real tasks do
  return Array.from({ length: n }, (_, i) =>
    INBOX[i] ?? {
      taskId: `WP-${188100 + i}`,
      title:
        i % 3 === 2
          ? `■ Demo ■ [Bug] 共有フォルダの権限変更で403 insufficient_scopeエラーが発生する問題の調査・対応 / Điều tra lỗi 403 khi đổi quyền thư mục (${i + 1})`
          : `Task demo số ${i + 1}: sửa lỗi hiển thị danh sách`,
      project: "Web Platform",
    },
  );
})();

const DISTANT: DistantSession[] = (() => {
  const n = urlCount("distant");
  if (n === undefined) return BASE_DISTANT;
  return Array.from({ length: n }, (_, i) => ({
    id: `ext-${i + 1}`,
    agent: i % 3 === 1 ? "codex" : "claude",
    label: BASE_DISTANT[i]?.label ?? `repo-${i + 1}`,
    status: STATUSES[i % 3] ?? "idle",
  }));
})();

// ---------- quota connections ----------

/** `?link=claude-signedOut,codex-error` shows those problems in the browser preview. */
const mockLinkState: Record<Provider, { status: LinkStatus; okAt?: number }> = (() => {
  const out: Record<Provider, { status: LinkStatus; okAt?: number }> = {
    claude: { status: "ok" },
    codex: { status: "ok" },
  };
  if (typeof window === "undefined") return out;
  for (const part of (new URLSearchParams(window.location.search).get("link") ?? "").split(",")) {
    const [p, status] = part.split("-");
    if ((p === "claude" || p === "codex") && (status === "error" || status === "missing" || status === "signedOut")) {
      out[p] = { status };
    }
  }
  return out;
})();

const MOCK_LINK_MESSAGE: Partial<Record<LinkStatus, (p: Provider) => string>> = {
  error: (p) =>
    p === "codex"
      ? "codex app-server lỗi: exited. App tự thử lại sau 1 phút, hoặc bấm Kết nối lại."
      : "Probe Claude lỗi: timeout. App thử lại sau 30 phút, hoặc bấm Kết nối lại.",
  missing: (p) => `Không thấy lệnh ${p} trên PATH. Cài ${p === "codex" ? "Codex CLI" : "Claude Code"} rồi bấm Kết nối lại.`,
  signedOut: (p) =>
    `${p === "codex" ? "Codex" : "Claude"} chưa đăng nhập hoặc phiên đăng nhập đã hết hạn. Bấm Đăng nhập để mở cửa sổ đăng nhập.`,
};

export function mockLinks(now: number): ProviderLink[] {
  return (["claude", "codex"] as const).map((provider) => {
    const s = mockLinkState[provider];
    if (s.okAt !== undefined && now >= s.okAt) mockLinkState[provider] = { status: "ok" };
    const { status } = mockLinkState[provider];
    const message = MOCK_LINK_MESSAGE[status]?.(provider);
    const link: ProviderLink = { provider, status, since: iso(now - 4 * 60_000) };
    if (message) link.message = message;
    return link;
  });
}

/** Browser preview: "Kết nối lại" or a finished sign-in turns the link green after a moment. */
export function mockReconnect(provider?: Provider): null {
  for (const p of ["claude", "codex"] as const) {
    if (!provider || provider === p) mockLinkState[p] = { status: "connecting", okAt: Date.now() + 1500 };
  }
  return null;
}

export function mockAppInfo(): AppInfo {
  return { version: "0.2.0-demo", core: "mock" };
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

/** A second task in "Đang làm" that only went through intake (several tasks can run at once). */
const SECOND = {
  id: "WP-188003",
  wpId: 188003,
  title: "Tối ưu truy vấn danh sách thư mục dùng chung",
  project: "Web Platform",
  repo: "acme-api",
  branch: "f/WP-188003",
};

function secondSystem(now: number): TaskSystem {
  const started = iso(now - 25 * 60_000);
  mockRegisterFlow(SECOND.wpId, SECOND.id, SECOND.title);
  const flow = mockFlowSystem(SECOND.wpId);
  const system: TaskSystem = {
    taskId: SECOND.id,
    title: SECOND.title,
    repo: SECOND.repo,
    branch: SECOND.branch,
    nxStatus: "In progress",
    nxStatusColor: "#cc5de8",
    startedAt: started,
    steps: STEP_ORDER.map((kind, i) =>
      i === 0
        ? { kind, index: 1, state: "done", startedAt: started, endedAt: started }
        : { kind, index: i + 1, state: "pending" },
    ),
    agents: [],
    findings: [],
    newComments: 0,
  };
  return flow ? { ...system, ...flow } : system;
}

export function orbitForFrame(frameIndex: number, now: number): OrbitSnapshot {
  const f = frameAt(frameIndex);
  const quota = mockQuota(frameIndex, now);
  if (!f.active) return { inbox: INBOX_VIEW, distant: DISTANT, quota };
  const active: TaskSystem = {
    taskId: ACTIVE_ID,
    title: "Không tải được danh sách file khi user chưa có org",
    repo: "acme-web",
    branch: "feature/187716",
    nxStatus: "In progress",
    nxStatusColor: "#cc5de8",
    startedAt: iso(now - frameIndex * FRAME_TIME_MS),
    steps: buildSteps(f, frameIndex, now),
    agents: buildAgents(f, now),
    findings: buildFindings(f),
    newComments: f.newComments,
  };
  if (f.gate) active.gate = { kind: f.gate.kind, since: iso(now - 40_000), message: f.gate.message };
  return { active, others: [secondSystem(now)], inbox: INBOX_VIEW, distant: DISTANT, quota };
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
    nxStatusColor: "#cc5de8",
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
    { id: "WP-188001", wpId: 188001, title: INBOX[0]?.title ?? "", project: "Web Platform", nxStatus: "Open", nxStatusColor: "#1a67a3", column: "inbox", newComments: 2 },
    { id: "WP-188002", wpId: 188002, title: INBOX[1]?.title ?? "", project: "Mobile App", nxStatus: "Reopened", nxStatusColor: "#ff3300", column: "inbox", newComments: 0 },
    activeCard,
    {
      id: "WP-186196",
      wpId: 186196,
      title: "Sửa thông báo lỗi khi token Box hết hạn",
      project: "Web Platform",
      repo: "acme-api",
      branch: "feature/186196",
      nxStatus: "In progress",
    nxStatusColor: "#cc5de8",
      column: "awaitingPush",
      step: { kind: "pushSummary", index: STEP_COUNT, state: "done", since: iso(now - 50 * 60_000) },
      agent: "claude",
      costUsd: 1.37,
      newComments: 0,
    },
  ];
  if (f.active) {
    mockRegisterFlow(SECOND.wpId, SECOND.id, SECOND.title);
    tasks.push({
      id: SECOND.id,
      wpId: SECOND.wpId,
      title: SECOND.title,
      project: SECOND.project,
      repo: SECOND.repo,
      branch: SECOND.branch,
      nxStatus: "In progress",
    nxStatusColor: "#cc5de8",
      column: "active",
      step: { kind: "intake", index: 1, state: "done", since: iso(now - 25 * 60_000) },
      newComments: 0,
      ...mockFlowCard(SECOND.wpId),
    });
  }
  const board: BoardSnapshot = {
    tasks,
    activeTaskIds: f.active ? [ACTIVE_ID, SECOND.id] : [],
    quota: mockQuota(frameIndex, now),
    links: mockLinks(now),
    today: { tokens: 1_240_000 + frameIndex * 18_000, costUsd: 0.84 + f.cost },
    mode: "demo",
    tasksDirOffer: mockTasksDirOffer(),
  };
  return board;
}

export const mockOrbit = (now: number): OrbitSnapshot => orbitForFrame(demoFrameIndex(now), now);

// ---------- settings (browser preview only; nothing is stored) ----------

let mockTokenSet = false;
let mockBaseUrl = "";

export function mockSettings(): SettingsView {
  return {
    nxBaseUrl: mockBaseUrl,
    nxTokenSet: mockTokenSet,
    workspaces: ["~/Desktop"],
    projects: [
      { name: "Web Platform", repos: ["~/Desktop/acme-web", "~/Desktop/acme-api"], branch: "f/WP-{taskId}" },
      { name: "Mobile App", repos: ["~/Desktop/acme-mobile"] },
    ],
    configPath: "C:/Users/me/.op-workflow-dev/config.json",
    dataDir: "C:/Users/me/.op-workflow-dev",
    browserDebug: "Chrome",
    tasksDir: mockTasksDir ?? "C:/Users/me/.op-workflow-dev/tasks",
    tasksDirChosen: mockTasksDir !== undefined,
    tasksDirSuggested: MOCK_DESKTOP_TASKS,
  };
}

const MOCK_DESKTOP_TASKS = "C:/Users/me/Desktop/op-workflow-tasks";
let mockTasksDir: string | undefined;

/** Browser preview: remembers the choice in memory only. */
export function mockSetTasksDir(dir?: string): TasksDirChange {
  mockTasksDir = dir ?? "C:/Users/me/.op-workflow-dev/tasks";
  return { path: mockTasksDir, moved: dir ? 2 : 0, warnings: [] };
}

/** `?offer` in the preview URL shows the one-time "save briefs on the Desktop" question. */
export const mockTasksDirOffer = (): string | undefined =>
  mockTasksDir === undefined && typeof location !== "undefined" && new URLSearchParams(location.search).has("offer")
    ? MOCK_DESKTOP_TASKS
    : undefined;

export function mockFetchIssues(wpId: number): IssueFetch {
  void wpId;
  return { read: 1, warnings: [] };
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

// ---------- start work (browser preview only; no git runs) ----------

interface MockRepo {
  name: string;
  remote: string;
  dirty: string[];
  /** [name, minutes since last commit, track] */
  branches: [string, number, string?][];
  lastBase?: string;
}

const MOCK_REPOS: MockRepo[] = [
  {
    name: "acme-web",
    remote: "git@github.com:acme/web.git",
    dirty: [],
    lastBase: "origin/development",
    branches: [
      ["origin/development", 120],
      ["origin/production", 3 * 24 * 60],
      ["f/WP-187641", 26 * 60, "ahead 2"],
      ["f/WP-187716", 40],
      ["origin/f/WP-186196", 2 * 24 * 60],
      ["development", 5 * 24 * 60, "behind 14"],
    ],
  },
  {
    name: "acme-api",
    remote: "https://gitlab.example.com/acme/api.git",
    dirty: [" M src/routes/files.ts", "?? notes.txt"],
    branches: [
      ["origin/development", 300],
      ["origin/production", 6 * 24 * 60],
      ["feature/186196", 50],
    ],
  },
  { name: "acme-tools", remote: "git@github.com:acme/tools.git", dirty: [], branches: [["origin/main", 9 * 24 * 60]] },
];

const mockPath = (name: string) => `C:\\code\\${name}`;

export function mockStartOptions(wpId: number): StartOptions {
  const card = mockBoard(Date.now()).tasks.find((t) => t.wpId === wpId);
  return {
    wpId,
    taskId: card?.id ?? `WP-${wpId}`,
    title: card?.title ?? "Task demo",
    project: card?.project ?? "Web Platform",
    nxStatus: card?.nxStatus ?? "Open",
    repos: MOCK_REPOS.map((r, i) => ({
      path: mockPath(r.name),
      name: r.name,
      remote: r.remote,
      suggested: i === 0,
      remembered: i === 1,
    })),
    repo: mockPath("acme-web"),
    branch: `feature/${wpId}`,
    branchSource: "default",
    issueLinks: ["https://github.com/acme/web/issues/456"],
    workspaces: ["~/Desktop"],
  };
}

export function mockRepoState(path: string, wpId?: number): RepoState {
  const repo = MOCK_REPOS.find((r) => mockPath(r.name) === path);
  if (!repo) throw { code: "repo.notGit", message: `${path} không phải repo git` };
  const now = Date.now();
  return {
    path,
    name: repo.name,
    branches: repo.branches
      .filter(([name]) => !mockDeleted.has(`${path}|${name}`))
      .map(([name, ageMin, track]) => ({
        name,
        remote: name.startsWith("origin/"),
        committedAt: new Date(now - ageMin * 60_000).toISOString(),
        track,
        protected: MOCK_PROTECTED.has(name.replace(/^origin\//, "")) || undefined,
      })),
    current: repo.branches.find(([n]) => !n.startsWith("origin/"))?.[0],
    dirty: repo.dirty,
    dirtyCount: repo.dirty.length,
    lastBase: repo.lastBase,
    suggestedBranch: wpId !== undefined && repo.name === "acme-web" ? `f/WP-${wpId}` : undefined,
  };
}

export const mockFetchRepo = (path: string, wpId?: number): RepoState => mockRepoState(path, wpId);

const MOCK_PROTECTED = new Set(["main", "master", "develop", "development", "production", "staging", "release"]);
/** Branches deleted in the browser preview (memory only). */
const mockDeleted = new Set<string>();

/** Browser preview: hides the branch; a local branch named *wip* pretends to be unmerged. */
export function mockDeleteBranch(req: DeleteBranchRequest): RepoState {
  const short = req.branch.replace(/^origin\//, "");
  if (MOCK_PROTECTED.has(short)) throw { code: "branch.protected", message: `Không xoá: ${short} là nhánh chính` };
  if (!req.remote && !req.force && short.includes("wip")) {
    throw { code: "branch.notMerged", message: "Nhánh còn commit chưa merge vào đâu." };
  }
  mockDeleted.add(`${req.repo}|${req.remote ? `origin/${short}` : short}`);
  return mockRepoState(req.repo, req.wpId);
}

export function mockStartTask(req: StartRequest): StartResult {
  const repo = MOCK_REPOS.find((r) => mockPath(r.name) === req.repo);
  if (repo && repo.dirty.length > 0) {
    throw { code: "repo.dirty", message: `Working tree có ${repo.dirty.length} file chưa commit.` };
  }
  return {
    taskId: `WP-${req.wpId}`,
    branch: req.branch,
    created: !repo?.branches.some(([n]) => n === req.branch),
    warnings: ["Bản demo trong trình duyệt: không tạo branch thật."],
    issueLinks: 0,
  };
}

export function mockUpdate(): UpdateView {
  return {
    current: "0.2.0",
    enabled: true,
    latest: "0.3.0",
    available: true,
    url: "https://github.com/wangnguen/op-workflow/releases",
    publishedAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
    checkedAt: new Date().toISOString(),
  };
}
export const mockBoard = (now: number): BoardSnapshot => boardForFrame(demoFrameIndex(now), now);
