// Mock data for browser mode, with a looping demo timeline so the UI animates.
// Every DEMO_FRAME_MS the active task advances one frame through the 8 steps.
import type {
  AgentCatalog,
  AiCli,
  AgentKind,
  AgentView,
  AppInfo,
  BoardSnapshot,
  CloseChoice,
  DeleteBranchRequest,
  DistantSession,
  FindingState,
  FindingView,
  GateKind,
  HarnessLib,
  InboxItem,
  IssueFetch,
  IssueStatus,
  LinkStatus,
  OrbitSnapshot,
  PriorityView,
  Provider,
  ProviderLink,
  QuotaSnapshot,
  RepoState,
  SessionStatus,
  SettingsView,
  StartOptions,
  StartRequest,
  StartResult,
  StepChoiceRequest,
  StepKind,
  StepModels,
  StepSetting,
  StepState,
  StepView,
  SubagentView,
  TaskCard,
  TasksDirChange,
  TaskSystem,
  UpdateView,
} from "./types";
import { AI_STEPS, OPTIONAL_STEPS, STEP_COUNT, STEP_ORDER, stepIndex } from "../shared/steps";
import { mockDoneCards, mockFlowCard, mockFlowSystem, mockMoved, mockPlaceCard, mockRegisterFlow, mockStarted } from "./mockFlow";
import { mockNewComments, mockNotify, mockUnreadAlerts } from "./mockTask";
import { mockOutside, mockOutsideHidden, mockSetOutsideHidden } from "./mockWorkspace";

export const DEMO_FRAME_MS = 4000;
/** One demo frame stands for this much "real" time in timestamps. */
const FRAME_TIME_MS = 150_000;
const T0 = Date.now();

export { STEP_ORDER };

const FIRST_FRAME: Record<StepKind, number> = {
  intake: 0,
  analyze: 1,
  implement: 3,
  checks: 7,
  harness: 7,
  reviewLocal: 7,
  fix: 8,
  crossReview: 9,
  fixCross: 11,
  browserTest: 13,
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
  /** the step the demo stands on; earlier steps are done (the optional ones passed) */
  step: StepKind;
  state: StepState;
  overrides?: Partial<Record<StepKind, StepState>>;
  round?: number;
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
  { active: true, step: "intake", state: "running", findings: {}, newComments: 0, cost: 0 },
  {
    active: true, step: "analyze", state: "running", findings: {}, newComments: 0, cost: 0.05,
    agent: { kind: "claude", status: "running", ctxPct: 8, compactions: 0, subagents: [sub("Explore")], tools: ["Grep", "Read", "Glob"] },
  },
  {
    active: true, step: "analyze", state: "waitingUser", findings: {}, newComments: 0, cost: 0.09,
    gate: { kind: "planReview", message: "plan.md đã sẵn sàng: 2 phương án, đề xuất phương án A" },
    agent: { kind: "claude", status: "waiting", ctxPct: 14, compactions: 0, subagents: [sub("Explore", "done")], tools: ["Write", "Read", "Grep"] },
  },
  {
    active: true, step: "implement", state: "running", findings: {}, newComments: 0, cost: 0.18,
    agent: { kind: "claude", status: "running", ctxPct: 31, compactions: 0, subagents: [sub("Explore")], tools: ["Edit", "Read", "Grep"] },
  },
  {
    active: true, step: "implement", state: "running", findings: {}, newComments: 1, cost: 0.31,
    agent: { kind: "claude", status: "running", ctxPct: 58, compactions: 0, subagents: [sub("Explore", "done"), sub("general-purpose"), sub("Plan")], tools: ["Edit", "Bash", "Edit"] },
  },
  {
    active: true, step: "implement", state: "running", findings: {}, newComments: 1, cost: 0.42,
    agent: { kind: "claude", status: "running", ctxPct: 96, compactions: 0, subagents: [sub("general-purpose")], tools: ["Bash", "Edit", "Read"] },
  },
  {
    active: true, step: "implement", state: "running", findings: {}, newComments: 1, cost: 0.47,
    agent: { kind: "claude", status: "running", ctxPct: 22, compactions: 1, subagents: [], tools: ["Read", "Edit"] },
  },
  // first /review-local, then Claude fixes what it found before Codex looks
  {
    active: true, step: "reviewLocal", state: "running", findings: {}, newComments: 0, cost: 0.53,
    agent: { kind: "claude", status: "running", ctxPct: 24, compactions: 1, subagents: [], tools: ["Bash", "Read", "Grep"] },
  },
  {
    active: true, step: "fix", state: "running", findings: { f6: "fixing" }, newComments: 0, cost: 0.58,
    agent: { kind: "claude", status: "running", ctxPct: 27, compactions: 1, subagents: [], tools: ["Edit", "Bash"] },
  },
  // Codex ⇄ Claude until Codex is clean
  {
    active: true, step: "crossReview", state: "running", round: 1, findings: { f6: "fixed" }, newComments: 0, cost: 0.62,
    agent: { kind: "claude", status: "idle", ctxPct: 27, compactions: 1, subagents: [], child: "running", tools: ["Bash"] },
  },
  {
    active: true, step: "crossReview", state: "running", round: 1, findings: { f6: "fixed", ...ALL_OPEN }, newComments: 0, cost: 0.67,
    agent: { kind: "claude", status: "idle", ctxPct: 27, compactions: 1, subagents: [], child: "running", tools: ["Bash"] },
  },
  {
    active: true, step: "fixCross", state: "running", round: 1,
    findings: { f6: "fixed", ...ALL_OPEN, f1: "fixing", f2: "fixing" }, newComments: 0, cost: 0.74,
    agent: { kind: "claude", status: "running", ctxPct: 35, compactions: 1, subagents: [], tools: ["Edit", "Edit", "Bash"] },
  },
  // every fix asks before Codex looks again
  {
    active: true, step: "fixCross", state: "waitingUser", round: 1,
    findings: { f6: "fixed", ...ALL_OPEN, f1: "fixing", f2: "fixing" }, newComments: 0, cost: 0.74,
    gate: { kind: "afterFix", message: "Sửa lỗi xong. Chạy Review chéo lại (vòng 2)? Đã review chéo 1 vòng." },
    agent: { kind: "claude", status: "waiting", ctxPct: 35, compactions: 1, subagents: [], tools: ["Bash"] },
  },
  {
    active: true, step: "crossReview", state: "running", round: 2, findings: { f6: "fixed", ...AFTER_R2 }, newComments: 0, cost: 0.81,
    agent: { kind: "claude", status: "idle", ctxPct: 37, compactions: 1, subagents: [], child: "running", tools: ["Bash"] },
  },
  // final /review-local, its findings wait for the user to pick, then push-summary
  {
    active: true, step: "finalReview", state: "running", round: 2, findings: { f6: "fixed", ...AFTER_R2, f5: "open" }, newComments: 0, cost: 0.88,
    agent: { kind: "claude", status: "running", ctxPct: 41, compactions: 1, subagents: [], tools: ["Bash", "Read"] },
  },
  {
    active: true, step: "finalReview", state: "waitingUser", round: 2, findings: { f6: "fixed", ...AFTER_R2, f5: "open", f7: "open" }, newComments: 0, cost: 0.92,
    gate: { kind: "findings", message: "Review chốt báo lỗi: 2 lỗi đang mở, đã chọn 0 cho AI sửa. Chọn lỗi cần sửa, hoặc sang Push summary mà không sửa." },
    agent: { kind: "claude", status: "waiting", ctxPct: 43, compactions: 1, subagents: [], tools: ["Bash"] },
  },
  {
    active: true, step: "pushSummary", state: "running", round: 2, findings: { f6: "fixed", ...AFTER_R2, f5: "deferred", f7: "accepted" }, newComments: 0, cost: 0.98,
    agent: { kind: "claude", status: "running", ctxPct: 45, compactions: 1, subagents: [], tools: ["Bash", "Bash"] },
  },
  { active: false, step: "pushSummary", state: "done", findings: {}, newComments: 0, cost: 1.04 },
  { active: false, step: "pushSummary", state: "done", findings: {}, newComments: 0, cost: 1.04 },
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

/** OpenProject's usual order, lowest first; only some have a colour, as on a real server. */
const PRIORITIES: PriorityView[] = [
  { name: "Low", rank: 1, ranks: 4 },
  { name: "Normal", rank: 2, ranks: 4 },
  { name: "High", color: "#ff922b", rank: 3, ranks: 4 },
  { name: "Immediate", color: "#e03131", rank: 4, ranks: 4 },
];

const INBOX: InboxItem[] = [
  {
    taskId: "WP-188001",
    wpId: 188001,
    title: "Lỗi đồng bộ quyền khi đổi owner thư mục",
    project: "Web Platform",
    nxStatus: "Open",
    nxStatusColor: "#1a67a3",
    priority: PRIORITIES[1],
  },
  {
    taskId: "WP-188002",
    wpId: 188002,
    title: "Thêm bộ lọc ngày cho báo cáo chia sẻ",
    project: "Mobile App",
    nxStatus: "Reopened",
    nxStatusColor: "#ff3300",
    priority: PRIORITIES[3],
  },
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
      wpId: 188100 + i,
      nxStatus: i % 2 === 0 ? "New" : "In progress",
      priority: PRIORITIES[i % PRIORITIES.length],
      title:
        i % 3 === 2
          ? `■ Demo ■ [Bug] 共有フォルダの権限変更で403 insufficient_scopeエラーが発生する問題の調査・対応 / Điều tra lỗi 403 khi đổi quyền thư mục (${i + 1})`
          : `Task demo số ${i + 1}: sửa lỗi hiển thị danh sách`,
      project: "Web Platform",
    },
  );
})();

/** The far stars: the mock sessions opened outside the app (mockWorkspace), or N made-up ones. */
function distant(): DistantSession[] {
  const base: DistantSession[] = mockOutside().map((o) => ({
    id: o.id,
    agent: "claude",
    label: o.cwd.split("/").pop() ?? o.cwd,
    status: o.state === "running" || o.state === "waiting" ? o.state : "idle",
    title: o.title,
    cwd: o.cwd,
    openIn: o.openIn,
  }));
  const n = urlCount("distant");
  if (n === undefined || mockOutsideHidden()) return base;
  return Array.from({ length: n }, (_, i) => ({
    id: `ext-${i + 1}`,
    agent: "claude",
    label: base[i]?.label ?? `repo-${i + 1}`,
    status: STATUSES[i % 3] ?? "idle",
  }));
}

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
  return { version: "0.4.0-demo", core: "mock" };
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
  const at = STEP_ORDER.indexOf(f.step);
  return STEP_ORDER.map((kind, i) => {
    // Kiểm tra, Harness and Test trình duyệt are not turned on yet: the flow passes them
    const passed = OPTIONAL_STEPS.includes(kind) ? "skipped" : "done";
    let state: StepState = i < at ? passed : i === at ? f.state : "pending";
    const override = f.overrides?.[kind];
    if (override) state = override;
    const view: StepView = { kind, index: i + 1, state };
    if (state !== "pending" && state !== "skipped") {
      const started = taskStart + FIRST_FRAME[kind] * FRAME_TIME_MS;
      view.startedAt = iso(started);
      if (state === "done") view.endedAt = iso(started + FRAME_TIME_MS * 0.9);
      view.costUsd = Math.round((0.04 + i * 0.03) * 100) / 100;
    }
    // Codex runs two rounds; the second Sửa lỗi fixes what round 1 found
    if (kind === "crossReview" && state !== "pending") view.round = f.round ?? 1;
    return view;
  });
}

function buildAgents(f: Frame, now: number): AgentView[] {
  const spec = f.agent;
  if (!spec) return [];
  const step = f.step;
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
  if (!f.active) return { inbox: INBOX_VIEW, distant: distant(), quota };
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
  return { active, others: [secondSystem(now)], inbox: INBOX_VIEW, distant: distant(), quota };
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
      kind: f.step,
      index: stepIndex(f.step),
      state: f.state,
      since: iso(now - 60_000),
    },
    costUsd: f.cost,
    newComments: f.newComments,
  };
  if (runningAgent) activeCard.agent = runningAgent;
  if (f.agent) activeCard.contextPct = f.agent.ctxPct;

  const tasks: TaskCard[] = [
    { id: "WP-188001", wpId: 188001, title: INBOX[0]?.title ?? "", project: "Web Platform", nxStatus: "Open", nxStatusColor: "#1a67a3", column: "inbox", newComments: mockNewComments(188001), priority: { name: "Normal" } },
    { id: "WP-188002", wpId: 188002, title: INBOX[1]?.title ?? "", project: "Mobile App", nxStatus: "Reopened", nxStatusColor: "#ff3300", column: "inbox", newComments: 0, priority: { name: "Immediate", color: "#e03131" } },
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
  if (f.active || mockMoved(SECOND.wpId)) {
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
      newComments: mockNewComments(SECOND.wpId),
      priority: { name: "High", color: "#ff922b" },
      ...mockFlowCard(SECOND.wpId),
    });
  }
  tasks.push(...mockDoneCards(urlCount("done")));
  for (const t of tasks) {
    const status = mockIssueStatus.get(t.wpId);
    if (status) t.issues = status;
  }
  const board: BoardSnapshot = {
    // what the user did on the preview board (start, pause, finish, "Làm tiếp") wins over the demo
    tasks: tasks.map(mockPlaceCard),
    activeTaskIds: f.active ? [ACTIVE_ID, SECOND.id] : [],
    quota: mockQuota(frameIndex, now),
    links: mockLinks(now),
    today: { tokens: 1_240_000 + frameIndex * 18_000, costUsd: 0.84 + f.cost },
    mode: mockNoToken() ? "noToken" : "demo",
    notice: mockNoToken()
      ? "Chưa có token OpenProject nên chưa có task giao cho bạn. Workspace và Phiên vẫn dùng bình thường; muốn nhận task thì mở Cài đặt → OpenProject."
      : undefined,
    unreadAlerts: mockUnreadAlerts(),
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
    browser: mockBrowser(),
    tasksDir: mockTasksDir ?? MOCK_DESKTOP_TASKS,
    tasksDirChosen: mockTasksDir !== undefined,
    tasksDirSuggested: MOCK_DESKTOP_TASKS,
    notify: mockNotify(),
    steps: mockStepSettings(),
    verifyMcp: mockVerifyMcp,
    claudeActiveProbeSec: mockProbe.activeSec,
    claudeProbeMin: mockProbe.idleMin,
    onClose: mockClose.choice,
    hideOutsideSessions: mockOutsideHidden(),
    harnessLibs: mockHarnessLibs(),
    repoChecks: mockRepos.map((repo) => ({ repo, commands: mockChecks.get(repo) ?? [] })),
  };
}

const mockRepos = ["~/Desktop/acme-web", "~/Desktop/acme-api", "~/Desktop/acme-mobile"];
const mockChecks = new Map<string, string[]>([["~/Desktop/acme-web", ["npx tsc --noEmit -p web", "npx eslint {files}"]]]);

/** Browser preview: Settings → Repo → Lệnh kiểm tra, in memory only. */
export function mockSetRepoChecks(repo: string, commands: string[]): SettingsView {
  const list = commands.map((c) => c.trim()).filter(Boolean);
  if (list.length) mockChecks.set(repo, list);
  else mockChecks.delete(repo);
  return mockSettings();
}

const mockHarnessDirs = new Map<string, string>();

function mockHarnessLibs(): HarnessLib[] {
  return mockRepos.map((repo) => {
    const custom = mockHarnessDirs.get(repo);
    const name = repo.split("/").pop() ?? "repo";
    return { repo, dir: custom ?? `C:\\tmp\\${name}-harness`, custom: custom !== undefined, exists: repo.endsWith("web") };
  });
}

/** Browser preview: Settings → Repo & nhánh → Thư viện harness, in memory only. */
export function mockSetHarnessDir(repo: string, dir?: string): SettingsView {
  const d = dir?.trim();
  if (d && !/^([a-zA-Z]:[\\/]|~[\\/]|\/)/.test(d)) {
    throw { code: "harness.dir", message: "Nhập đường dẫn đầy đủ, ví dụ C:\\tmp\\web-harness" };
  }
  if (d) mockHarnessDirs.set(repo, d);
  else mockHarnessDirs.delete(repo);
  return mockSettings();
}

const mockClose: { choice: CloseChoice } = { choice: "ask" };

/** Browser preview: Settings → Thông báo → "Khi đóng cửa sổ", in memory only. */
export function mockSetOnClose(choice: CloseChoice): SettingsView {
  mockClose.choice = choice;
  return mockSettings();
}

/** Browser preview: Settings → Phiên → "Hiện phiên mở ngoài app", in memory only. */
export function mockSetHideOutside(hide: boolean): SettingsView {
  mockSetOutsideHidden(hide);
  return mockSettings();
}

const mockProbe = { activeSec: 60, idleMin: 15 };

/** Browser preview: Settings → Hạn mức probe intervals, in memory only (same rules as the core). */
export function mockSetQuotaProbe(activeSec: number, idleMin: number): SettingsView {
  mockProbe.activeSec = activeSec <= 0 ? 0 : Math.max(30, Math.round(activeSec));
  mockProbe.idleMin = Math.max(1, Math.round(idleMin));
  return mockSettings();
}

const LEVELS = ["low", "medium", "high", "xhigh", "max"];

/** What the CLIs answer in the preview (shaped like the real lists, shortened).
 *  `?modelsError` shows failed reads: Claude with the last list it gave, Codex with none. */
export function mockStepModels(): StepModels {
  const readAt = new Date().toISOString();
  const ok = mockGoodModels(readAt);
  if (typeof location !== "undefined" && new URLSearchParams(location.search).has("modelsError")) {
    const yesterday = new Date(Date.now() - 26 * 3600 * 1000).toISOString();
    return {
      claude: { ...ok.claude, fromCli: false, error: "Claude CLI không trả lời sau 20 giây", readAt: yesterday, cliVersion: undefined },
      codex: {
        agent: "codex",
        models: [],
        defaultModel: "gpt-6.1-sol",
        defaultLabel: "gpt-6.1-sol",
        defaultEffort: "xhigh",
        fromCli: false,
        error: "Chưa cài Codex CLI (không thấy codex trên PATH)",
        modes: [],
      },
    };
  }
  return ok;
}

/** Permission modes as the CLIs' --help list them (Claude Code 2.1.326, Codex 0.169.0). */
const CLAUDE_MODES = ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];
const CODEX_MODES = ["read-only", "workspace-write", "danger-full-access", "approve-for-me", "dangerously-bypass-approvals-and-sandbox"];

function mockGoodModels(readAt: string): StepModels {
  return {
    claude: {
      agent: "claude",
      models: [
        { id: "claude-opus-5-5", label: "Opus 5.5", description: "For complex work and everyday tasks", efforts: LEVELS, isDefault: true },
        { id: "claude-sonnet-5-5", label: "Sonnet 5.5", description: "Most efficient for simpler tasks", efforts: LEVELS, isDefault: false },
        { id: "claude-fable-5-2", label: "Fable 5.2", description: "For your toughest challenges", efforts: LEVELS, isDefault: false },
        { id: "claude-haiku-5-5", label: "Haiku 5.5", description: "Fastest for quick answers", efforts: LEVELS, isDefault: false },
        { id: "claude-haiku-4-5-20251001", label: "Haiku 4.5", description: "Fastest for quick answers", efforts: [], isDefault: false },
        { id: "claude-opus-4-6", label: "Opus 4.6", efforts: ["low", "medium", "high", "max"], isDefault: false },
      ],
      defaultModel: "opus[1m]",
      defaultId: "claude-opus-5-5",
      defaultLabel: "Opus 5.5 · 1M",
      defaultEffort: "high",
      fromCli: true,
      cliVersion: "2.1.325",
      readAt,
      modes: CLAUDE_MODES,
    },
    codex: {
      agent: "codex",
      models: [
        { id: "gpt-6.1-sol", label: "GPT-6.1-Sol", description: "Latest workhorse model", efforts: [...LEVELS, "ultra"], defaultEffort: "low", isDefault: true },
        { id: "gpt-6-luna", label: "GPT-6-Luna", description: "Fast and affordable", efforts: LEVELS, defaultEffort: "medium", isDefault: false },
      ],
      defaultModel: "gpt-6.1-sol",
      defaultId: "gpt-6.1-sol",
      defaultLabel: "GPT-6.1-Sol",
      defaultEffort: "xhigh",
      fromCli: true,
      cliVersion: "0.169.0",
      readAt,
      modes: CODEX_MODES,
    },
  };
}

/** Past runs the preview shows next to each step: [runs, avg cost, avg seconds]. */
const MOCK_STEP_HISTORY: Partial<Record<StepKind, [number, number | undefined, number]>> = {
  analyze: [3, 0.24, 52],
  implement: [2, 0.49, 54],
  reviewLocal: [3, 0.55, 95],
  fix: [2, 0.31, 40],
  crossReview: [3, undefined, 280],
  finalReview: [2, 0.62, 126],
  pushSummary: [2, 0.09, 14],
};
const mockPicks: Partial<Record<StepKind, Omit<StepChoiceRequest, "step">>> = {};
let mockVerifyMcp = true;
const mockLimits: Partial<Record<StepKind, { maxBudgetUsd?: number; timeoutMin?: number }>> = {};
const DEFAULT_LIMITS: Partial<Record<StepKind, [number, number]>> = {
  analyze: [5, 20], implement: [5, 60], harness: [2, 30], reviewLocal: [3, 30],
  fix: [2, 45], crossReview: [3, 30], browserTest: [3, 30], finalReview: [3, 30], pushSummary: [1, 10],
};
export function mockSetVerifyMcp(on: boolean): SettingsView {
  mockVerifyMcp = on;
  return mockSettings();
}
export function mockSetStepLimits(limits: { step: StepKind; maxBudgetUsd?: number; timeoutMin?: number }): SettingsView {
  if (!AI_STEPS.includes(limits.step) ||
      (limits.maxBudgetUsd !== undefined && (!Number.isFinite(limits.maxBudgetUsd) || limits.maxBudgetUsd <= 0)) ||
      (limits.timeoutMin !== undefined && (!Number.isInteger(limits.timeoutMin) || limits.timeoutMin <= 0 || limits.timeoutMin > 4294967295))) {
    throw { code: "step.limits", message: "Trần chi phí và thời gian phải lớn hơn 0; để trống để dùng mặc định." };
  }
  mockLimits[limits.step] = { maxBudgetUsd: limits.maxBudgetUsd, timeoutMin: limits.timeoutMin };
  return mockSettings();
}

/** Same rules as the core (`flow::agent_for`): the pick, else Claude; Review chéo takes the other
 *  AI than Viết code's; Test trình duyệt stays on Claude. */
function mockAgentDefault(step: StepKind): AgentKind {
  if (step !== "crossReview") return "claude";
  return mockAgentFor("implement") === "codex" ? "claude" : "codex";
}
function mockAgentFor(step: StepKind): AgentKind {
  if (step === "browserTest") return "claude";
  return mockPicks[step]?.agent ?? mockAgentDefault(step);
}
function mockPermissionDefault(step: StepKind, agent: AgentKind): string | undefined {
  const writes = step === "implement" || step === "fix" || step === "harness";
  if (agent === "codex") return writes ? "workspace-write" : "read-only";
  return writes ? "acceptEdits" : undefined;
}

function mockStepSettings(): StepSetting[] {
  return AI_STEPS.map((step) => {
    const pick = mockPicks[step] ?? {};
    const agent = mockAgentFor(step);
    const [runs, avgCostUsd, avgSeconds] = MOCK_STEP_HISTORY[step] ?? [0, undefined, 0];
    return {
      step,
      agent,
      agentChoice: step !== "browserTest",
      agentDefault: mockAgentDefault(step),
      permissionMode: pick.permissionMode,
      permissionDefault: mockPermissionDefault(step, agent),
      model: pick.model,
      effort: pick.effort,
      effortDefault: step === "pushSummary" && agent === "claude" ? "low" : undefined,
      runs,
      ...mockLimits[step],
      defaultBudgetUsd: DEFAULT_LIMITS[step]?.[0],
      defaultTimeoutMin: DEFAULT_LIMITS[step]?.[1],
      avgCostUsd,
      avgSeconds: runs > 0 ? avgSeconds : undefined,
      lastModel: runs > 0 ? (agent === "codex" ? "gpt-6.1-sol" : "claude-opus-5-5") : undefined,
    };
  });
}

/** Browser preview: remembers the pick in memory only. */
export function mockSetStepChoice(choice: StepChoiceRequest): SettingsView {
  if (choice.model && !/^[A-Za-z0-9._:/[\]-]+$/.test(choice.model)) {
    throw { code: "step.choice", message: `Tên model không hợp lệ: ${choice.model}` };
  }
  if (choice.step === "browserTest" && choice.agent === "codex") {
    throw { code: "step.choice", message: "Test trình duyệt chạy bằng Claude cho tới khi Codex qua một lần chạy thử thật" };
  }
  const agent = choice.agent ?? mockAgentFor(choice.step);
  const catalog = mockStepModels()[agent === "codex" ? "codex" : "claude"];
  // only listed models; a saved one the list no longer has may stay (same rule as the core)
  if (choice.model && catalog.fromCli && choice.model !== mockPicks[choice.step]?.model && !catalog.models.some((m) => m.id === choice.model)) {
    throw { code: "step.choice", message: `${agent === "codex" ? "Codex" : "Claude Code"} trên máy không có model ${choice.model}` };
  }
  if (choice.effort) {
    const problem = mockEffortProblem(catalog, choice.model, choice.effort);
    if (problem) throw { code: "step.choice", message: problem };
  }
  const mode = choice.permissionMode;
  if (mode && mode !== mockPicks[choice.step]?.permissionMode && catalog.modes.length > 0 && !catalog.modes.includes(mode)) {
    throw { code: "step.choice", message: `${agent === "codex" ? "Codex" : "Claude Code"} trên máy không có chế độ ${mode}` };
  }
  if (mode === "bypassPermissions" && choice.step === "analyze") {
    throw { code: "step.choice", message: "Phân tích chạy Claude ở chế độ chỉ đọc (--restricted), không nhận bypassPermissions" };
  }
  // only an agent other than the default is kept, so Review chéo goes on following Viết code
  const keep = choice.agent && choice.agent !== mockAgentDefault(choice.step) ? choice.agent : undefined;
  mockPicks[choice.step] = { agent: keep, model: choice.model, effort: choice.effort, permissionMode: mode };
  return mockSettings();
}

/** Same rule as the core's `models::check_effort`: the picked model (else the CLI's default)
 *  must take the effort; a model the list does not know takes any level some model takes. */
function mockEffortProblem(catalog: AgentCatalog, model: string | undefined, effort: string): string | undefined {
  if (!/^[a-z]{1,16}$/.test(effort)) return `Effort không hợp lệ: ${effort}`;
  if (!catalog.fromCli) return undefined;
  const id = model ?? catalog.defaultId;
  const entry = catalog.models.find((m) => m.id === id);
  if (entry) {
    if (entry.efforts.length === 0) return `${entry.label} không nhận effort`;
    if (entry.efforts.includes(effort)) return undefined;
    return `${entry.label} không có effort ${effort} (có: ${entry.efforts.join(", ")})`;
  }
  return catalog.models.some((m) => m.efforts.includes(effort)) ? undefined : `Không model nào của máy có effort ${effort}`;
}

const MOCK_DESKTOP_TASKS = "C:/Users/me/Desktop/op-workflow-tasks";
let mockTasksDir: string | undefined;

/** Browser preview: remembers the choice in memory only. */
export function mockSetTasksDir(dir?: string): TasksDirChange {
  mockTasksDir = dir ?? "C:/Users/me/.op-workflow-dev/tasks";
  return { path: mockTasksDir, moved: dir ? 2 : 0, warnings: [] };
}

const urlFlag = (name: string): string | null =>
  typeof location === "undefined" ? null : new URLSearchParams(location.search).get(name);

/** `?noToken` in the preview URL: the board without an OpenProject token. */
export const mockNoToken = (): boolean => urlFlag("noToken") !== null;

/** `?chrome=closed|notEnabled|missing` in the preview URL: Chrome in that state. */
function mockBrowser(): SettingsView["browser"] {
  const state = urlFlag("chrome");
  if (state === "closed" || state === "notEnabled" || state === "missing") return { state };
  return { state: "ready", name: "Chrome" };
}

/** Claude Code and Codex in the preview; `?noCodex` shows Codex not installed, `?codexOut`
 *  signed out. */
export function mockAiClis(): AiCli[] {
  const codex: AiCli =
    urlFlag("noCodex") !== null
      ? { provider: "codex", installWith: "npm install -g @openai/codex" }
      : {
          provider: "codex",
          path: "C:/Users/me/AppData/Roaming/npm/codex.exe",
          version: "codex-cli 0.169.1",
          signedIn: urlFlag("codexOut") === null,
          installWith: "npm install -g @openai/codex",
        };
  return [
    {
      provider: "claude",
      path: "C:/Users/me/.local/bin/claude.exe",
      version: "2.1.329 (Claude Code)",
      signedIn: true,
      installWith: "PowerShell: irm https://claude.ai/install.ps1 | iex",
    },
    codex,
  ];
}

const mockIssueStatus = new Map<number, IssueStatus>();

/** `?issueError` in the preview URL makes "Lấy issue" fail, to see the red state. */
export function mockFetchIssues(wpId: number): IssueFetch {
  const at = new Date().toISOString();
  if (typeof location !== "undefined" && new URLSearchParams(location.search).has("issueError")) {
    const message = "Chrome chưa bật cho phép kết nối (chrome://inspect/#remote-debugging)";
    mockIssueStatus.set(wpId, { at, read: 0, error: message });
    throw { code: "browser.notEnabled", message };
  }
  mockIssueStatus.set(wpId, { at, read: 1 });
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
  mockStarted(req.wpId, INBOX.find((i) => i.taskId === `WP-${req.wpId}`)?.title ?? `WP-${req.wpId}`, req.carryOn);
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
    current: "0.4.0",
    enabled: true,
    latest: "0.5.0",
    available: true,
    url: "https://github.com/wangnguen/op-workflow/releases",
    publishedAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
    checkedAt: new Date().toISOString(),
  };
}
export const mockBoard = (now: number): BoardSnapshot => boardForFrame(demoFrameIndex(now), now);
