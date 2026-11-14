// Browser preview of steps 2-8: a small in-memory workflow per task, each step "runs" for a few
// seconds. Mirrors core/orchestrator/src/flow.rs closely enough to try every button.
// `?flowError` in the URL makes the first Analyze fail with "not signed in".
import type {
  AgentKind,
  AgentView,
  FindingDecision,
  FindingView,
  FlowAction,
  FlowError,
  FlowStatus,
  GateKind,
  GateView,
  LogLine,
  PushView,
  StepKind,
  StepState,
  StepView,
  TaskCard,
  TaskDetail,
  ToolEvent,
} from "./types";
import { STEP_ORDER } from "../shared/steps";

/** How long a mock step "runs". */
export const MOCK_STEP_MS = 2600;
const MAX_ROUNDS = 3;

const LABEL: Record<StepKind, string> = {
  intake: "Nhận task",
  analyze: "Phân tích",
  implement: "Viết code",
  reviewLocal: "Tự review",
  fix: "Sửa lỗi",
  crossReview: "Review chéo",
  finalReview: "Review chốt",
  pushSummary: "Chuẩn bị push",
};

const TOOLS: Record<StepKind, string[]> = {
  intake: [],
  analyze: ["Grep orgId", "Read api/src/routes/files.ts", "Read api/src/services/org.ts"],
  implement: ["Read api/src/routes/files.ts", "Edit api/src/routes/files.ts", "Bash npx tsc --noEmit"],
  reviewLocal: ["Bash git diff origin/development", "Read api/src/routes/files.ts"],
  fix: ["Edit api/src/routes/files.ts", "Bash npx tsc --noEmit"],
  crossReview: ["Shell git merge-base origin/development HEAD", "Shell git diff 3f2a9c1"],
  finalReview: ["Bash git diff origin/development", "Read api/test/files.test.ts"],
  pushSummary: ["Bash git diff origin/development", "Bash git log -5"],
};

const COST: Record<StepKind, number> = {
  intake: 0,
  analyze: 0.31,
  implement: 1.12,
  reviewLocal: 0.64,
  fix: 0.38,
  crossReview: 0,
  finalReview: 0.58,
  pushSummary: 0.21,
};

const PLAN = `## Nguyên nhân
\`api/src/routes/files.ts:42\` đọc \`user.org.id\` mà không kiểm tra user đã có org. User mới (chưa được thêm vào org) làm route ném TypeError, nên danh sách file trả 500.

## Phương án
1. **Trả danh sách rỗng khi chưa có org** — ít sửa nhất; UI hiện "chưa có file". Nhược: che mất trường hợp dữ liệu hỏng.
2. **Trả 403 kèm mã \`org.missing\`** — rõ ràng hơn cho UI; phải sửa thêm trang web.

## Đề xuất
Phương án 1, kèm log cảnh báo để còn thấy dữ liệu hỏng.

## File sẽ sửa
- api/src/routes/files.ts
- api/test/files.test.ts

## Kế hoạch test
- Test mới: user không có org → 200 + danh sách rỗng.
- Chạy lại test hiện có của route.

## Rủi ro và câu hỏi
- Trang mobile có dựa vào lỗi 500 này không?`;

const PUSH = `### Commit message
\`\`\`text
fix(files): return an empty list for users without an org (#456)

- guard user.org before reading its id
- log a warning so broken data still shows up
- test the no-org case
\`\`\`

### PR description
\`\`\`markdown
## Summary
Users who are not in an org got a 500 on the file list.

## Changes
- Guard \`user.org\` in \`api/src/routes/files.ts\`
- New test for the no-org case

## Testing
- npm test -- files
\`\`\``;

const BRIEF = (id: string, title: string) => `# [${id}] ${title}
Task: https://openproject.example.com/work_packages/${id.replace(/\D/g, "")} · Repo: acme-api
Branch: f/${id} (base: origin/development)

## Yêu cầu
User chưa có org mở trang Files thì gặp lỗi 500.

## Comment trên OpenProject
- (mới) Khách báo lại lỗi này trên bản production.

## Từ issue
https://github.com/acme/api/issues/456
`;

interface Finding extends FindingView {
  /** first review that reports it (index into REVIEWS) */
}

/** What each review run "finds", by run number. */
const REVIEWS: Record<"reviewLocal" | "crossReview" | "finalReview", Omit<FindingView, "state">[][]> = {
  reviewLocal: [
    [
      { fingerprint: "m-high", severity: "high", title: "Null khi user chưa có org", file: "api/src/routes/files.ts", line: 42, source: "review-local", detail: "user.org có thể là null với user mới.", suggestion: "Kiểm tra user.org trước khi đọc id." },
      { fingerprint: "m-low", severity: "low", title: "Tên biến khó hiểu", file: "api/src/lib/util.ts", line: 10, source: "review-local" },
    ],
  ],
  crossReview: [[], []],
  finalReview: [
    [
      { fingerprint: "m-med", severity: "medium", title: "Thiếu log khi org bị xoá giữa chừng", file: "api/src/routes/files.ts", line: 51, source: "review-local", detail: "Khi org bị xoá trong lúc request chạy, route trả rỗng mà không log.", suggestion: "Thêm logger.warn kèm userId." },
    ],
    [],
  ],
};

interface Flow {
  wpId: number;
  taskId: string;
  title: string;
  states: Record<StepKind, StepState>;
  /** step that is ready, running, failed or waiting */
  current?: StepKind;
  phase: "ready" | "running" | "gate" | "awaitingPush" | "done";
  gate?: GateKind;
  gateSince: string;
  error?: FlowError;
  runEnds?: number;
  runStarted?: string;
  round: number;
  runs: Record<string, number>;
  auto: boolean;
  brief: string;
  plan?: string;
  push?: PushView;
  findings: Finding[];
  log: LogLine[];
  costs: Partial<Record<StepKind, number>>;
  failFirst: boolean;
}

const flows = new Map<number, Flow>();
/** wpId of the animated demo task in mock.ts. */
const DEMO_ACTIVE_WP = 187716;
const iso = (ms: number) => new Date(ms).toISOString();
const agentOf = (step: StepKind): AgentKind => (step === "crossReview" ? "codex" : "claude");
const settled = (f: FindingView) => f.state === "fixed" || f.state === "accepted" || f.state === "deferred";
const toFix = (f: FindingView) => !settled(f) && (f.chosen === true || f.severity !== "low");

function wantsError(): boolean {
  return typeof location !== "undefined" && new URLSearchParams(location.search).has("flowError");
}

/** The flow of a task, created at "Phân tích ready" the first time it is asked for. */
function flowOf(wpId: number, taskId = `WP-${wpId}`, title = "Task demo"): Flow {
  let f = flows.get(wpId);
  if (!f) {
    const states = Object.fromEntries(STEP_ORDER.map((s) => [s, "pending"])) as Record<StepKind, StepState>;
    states.intake = "done";
    states.analyze = "running";
    f = {
      wpId,
      taskId,
      title,
      states,
      current: "analyze",
      phase: "ready",
      gateSince: iso(Date.now()),
      round: 0,
      runs: {},
      auto: false,
      brief: BRIEF(taskId, title),
      findings: [],
      log: [{ at: iso(Date.now()), kind: "info", text: "Đã nhận task: tạo nhánh và brief xong" }],
      costs: {},
      failFirst: wantsError(),
    };
    if (wpId === DEMO_ACTIVE_WP) {
      // the animated demo task: open it where the demo shows a gate, at the plan review
      f.title = "Không tải được danh sách file khi user chưa có org";
      f.states.analyze = "waitingUser";
      f.plan = PLAN;
      toGate(f, "planReview", "analyze");
    }
    flows.set(wpId, f);
  }
  return f;
}

/** Called by the board mock so the preview's second task has a flow under its real name. */
export function mockRegisterFlow(wpId: number, taskId: string, title: string): void {
  flowOf(wpId, taskId, title);
}

function log(f: Flow, kind: LogLine["kind"], text: string, step?: StepKind): void {
  f.log.push({ at: iso(Date.now()), kind, text, step });
  if (f.log.length > 200) f.log.shift();
}

function goTo(f: Flow, step: StepKind): void {
  f.current = step;
  f.states[step] = "running";
  f.phase = "ready";
  f.gate = undefined;
  f.gateSince = iso(Date.now());
}

function toGate(f: Flow, kind: GateKind, step: StepKind): void {
  f.current = step;
  f.phase = "gate";
  f.gate = kind;
  f.gateSince = iso(Date.now());
}

function startRun(f: Flow): void {
  if (!f.current) return;
  f.phase = "running";
  f.error = undefined;
  f.runStarted = iso(Date.now());
  f.runEnds = Date.now() + MOCK_STEP_MS;
  // like the engine: what the fix takes is "being fixed" until the next review
  if (f.current === "fix") for (const x of f.findings) if (toFix(x)) x.state = "fixing";
  log(f, "info", `Bắt đầu ${LABEL[f.current]} (${agentOf(f.current) === "codex" ? "Codex" : "Claude"})`, f.current);
}

function merge(f: Flow, step: "reviewLocal" | "crossReview" | "finalReview", found: Omit<FindingView, "state">[]): void {
  const seen = new Set(found.map((x) => x.fingerprint));
  for (const x of found) {
    const old = f.findings.find((o) => o.fingerprint === x.fingerprint);
    const keep = old && (old.state === "accepted" || old.state === "deferred");
    const next: Finding = { ...x, state: keep ? old.state : "open", step, chosen: old?.chosen, reason: keep ? old.reason : undefined };
    if (step === "crossReview") next.round = f.round;
    if (old) Object.assign(old, next);
    else f.findings.push(next);
  }
  for (const o of f.findings) if (o.state === "fixing" && !seen.has(o.fingerprint)) o.state = "fixed";
}

/** The running step ends: same transitions as the Rust state machine. */
function complete(f: Flow): void {
  const step = f.current;
  if (!step) return;
  f.runEnds = undefined;
  for (const t of tools(f)) log(f, "tool", t.name, step);
  f.costs[step] = (f.costs[step] ?? 0) + COST[step];
  const n = (f.runs[step] = (f.runs[step] ?? 0) + 1);
  if (step === "analyze" && f.failFirst) {
    f.failFirst = false;
    f.states.analyze = "failed";
    f.error = {
      code: "agent.signedOut",
      message: "Claude chưa đăng nhập hoặc phiên đăng nhập đã hết hạn. Bấm Đăng nhập (mở `claude auth login`), xong thì bấm Thử lại.",
      provider: "claude",
    };
    toGate(f, "error", "analyze");
    log(f, "error", `${LABEL.analyze} lỗi: ${f.error.message}`, "analyze");
    return;
  }
  const done = (note: string) => log(f, "ok", `${LABEL[step]} xong: ${note}`, step);
  switch (step) {
    case "analyze":
      f.plan = PLAN;
      f.states.analyze = "waitingUser";
      toGate(f, "planReview", "analyze");
      done("plan.md");
      return;
    case "implement":
      f.states.implement = "done";
      goTo(f, "reviewLocal");
      done("$1.12");
      return;
    case "fix":
      f.states.fix = "done";
      f.round += 1;
      goTo(f, "crossReview");
      done(`${f.findings.filter((x) => x.state === "fixing").length} lỗi`);
      return;
    case "pushSummary":
      f.push = { issue: 456, text: PUSH, blocks: [] };
      f.push.blocks = [...PUSH.matchAll(/### (.+)\n```\w*\n([\s\S]*?)\n```/g)].map((m) => ({ label: m[1] ?? "", text: m[2] ?? "" }));
      f.states.pushSummary = "done";
      f.current = "pushSummary";
      f.phase = "awaitingPush";
      done("commit message + PR description");
      return;
    case "reviewLocal":
    case "crossReview":
    case "finalReview": {
      const found = REVIEWS[step][Math.min(n - 1, REVIEWS[step].length - 1)] ?? [];
      merge(f, step, found);
      const blocking = f.findings.filter(toFix).length;
      f.states[step] = "done";
      done(`báo ${found.length} lỗi; cần sửa ${blocking}`);
      if (step === "reviewLocal") {
        if (blocking > 0) goTo(f, "fix");
        else {
          f.round += 1;
          goTo(f, "crossReview");
        }
      } else if (step === "crossReview") {
        goTo(f, blocking > 0 && f.round < MAX_ROUNDS ? "fix" : "finalReview");
      } else {
        const high = f.findings.filter((x) => !settled(x) && (x.chosen || x.severity === "high" || x.severity === "critical")).length;
        const medium = f.findings.filter((x) => !settled(x) && !x.chosen && x.severity === "medium").length;
        if (high > 0) goTo(f, "fix");
        else if (medium > 0) {
          f.states.finalReview = "waitingUser";
          toGate(f, "mediumTriage", "finalReview");
        } else goTo(f, "pushSummary");
      }
      return;
    }
    default:
      return;
  }
}

/** Advance time: finish a run that is over, start the next step when auto-advance is on. */
function tick(f: Flow): void {
  if (f.phase !== "running" || f.runEnds === undefined || Date.now() < f.runEnds) return;
  complete(f);
  // complete() moved the phase on; TypeScript still narrows it to "running"
  if (f.auto && (f.phase as Flow["phase"]) === "ready") startRun(f);
}

function tools(f: Flow): ToolEvent[] {
  if (f.phase !== "running" || !f.current || !f.runStarted) return [];
  const started = Date.parse(f.runStarted);
  const shown = Math.min(TOOLS[f.current].length, 1 + Math.floor((Date.now() - started) / 700));
  return TOOLS[f.current].slice(0, shown).map((name, i) => ({ at: iso(started + i * 700), name }));
}

function steps(f: Flow): StepView[] {
  return STEP_ORDER.map((kind, i) => {
    let state = f.states[kind];
    if (state === "running" && !(f.phase === "running" && f.current === kind)) state = "pending";
    const view: StepView = { kind, index: i + 1, state };
    const cost = f.costs[kind];
    if (cost) view.costUsd = cost;
    if (kind === "crossReview" && f.round > 0) view.round = f.round;
    return view;
  });
}

function status(f: Flow): FlowStatus {
  return f.phase;
}

function gateView(f: Flow): GateView | undefined {
  if (f.phase !== "gate" || !f.gate) return undefined;
  const medium = f.findings.filter((x) => !settled(x) && !x.chosen && x.severity === "medium").length;
  const message: Record<GateKind, string> = {
    planReview: "Plan đã xong: duyệt để AI viết code, góp ý để phân tích lại, hoặc tự code.",
    mediumTriage: `${medium} lỗi medium cần bạn quyết: sửa, chấp nhận kèm lý do, hoặc để sau.`,
    selfCode: 'Bạn đang tự code trên nhánh này. Xong thì bấm "Tôi code xong" để app review.',
    agentQuestion: "AI đang hỏi bạn.",
    error: f.error?.message ?? "Bước vừa chạy bị lỗi.",
  };
  return { kind: f.gate, since: f.gateSince, message: message[f.gate] };
}

export function mockTaskDetail(wpId: number): TaskDetail {
  const f = flowOf(wpId);
  tick(f);
  const detail: TaskDetail = {
    wpId,
    taskId: f.taskId,
    title: f.title,
    project: "Web Platform",
    repo: "~/Desktop/acme-api",
    branch: `f/${f.taskId}`,
    base: "origin/development",
    status: status(f),
    current: f.current,
    steps: steps(f),
    gate: gateView(f),
    error: f.error,
    round: f.round,
    maxRounds: MAX_ROUNDS,
    autoAdvance: f.auto,
    brief: f.brief,
    plan: f.plan,
    findings: f.findings.map((x) => ({ ...x })),
    push: f.push,
    log: [...f.log, ...tools(f).map((t) => ({ at: t.at, kind: "tool" as const, text: t.name, step: f.current }))],
    costUsd: Object.values(f.costs).reduce((a, b) => a + (b ?? 0), 0),
  };
  if (f.phase === "running" && f.current && f.runStarted) {
    detail.running = { step: f.current, agent: agentOf(f.current), startedAt: f.runStarted, tools: tools(f), contextTokens: 182_000 };
  }
  return detail;
}

const fail = (code: string, message: string) => ({ code, message });

export function mockFlowAction(wpId: number, action: FlowAction): TaskDetail {
  const f = flowOf(wpId);
  tick(f);
  const running = f.phase === "running";
  switch (action.type) {
    case "run":
      if (running) throw fail("step.running", "Bước này đang chạy");
      if (f.phase === "gate" && f.gate === "error" && f.current) {
        f.states[f.current] = "running";
        f.phase = "ready";
      }
      if (f.phase !== "ready") throw fail("flow.notNow", "Lúc này chưa chạy bước tiếp được");
      startRun(f);
      break;
    case "retry":
      if (f.gate !== "error" || !f.current) throw fail("flow.notNow", "Lúc này chưa thử lại được");
      f.states[f.current] = "running";
      f.phase = "ready";
      log(f, "info", `Thử lại ${LABEL[f.current]}`, f.current);
      startRun(f);
      break;
    case "stop":
      if (!running || !f.current) throw fail("flow.notNow", "Không có bước nào đang chạy");
      f.runEnds = undefined;
      for (const x of f.findings) if (x.state === "fixing") x.state = "open";
      f.states[f.current] = "failed";
      f.error = { code: "step.stopped", message: "Bạn đã dừng bước này. Bấm Thử lại để chạy lại." };
      toGate(f, "error", f.current);
      log(f, "error", `${LABEL[f.current]} lỗi: ${f.error.message}`, f.current);
      break;
    case "approvePlan":
      if (f.gate !== "planReview") throw fail("flow.notNow", "Lúc này chưa duyệt plan được");
      f.states.analyze = "done";
      log(f, "ok", "Bạn duyệt plan: AI viết code", "analyze");
      goTo(f, "implement");
      startRun(f);
      break;
    case "rejectPlan":
      if (!action.feedback.trim()) throw fail("plan.feedback", "Ghi góp ý cho plan trước");
      if (f.gate !== "planReview") throw fail("flow.notNow", "Lúc này chưa góp ý plan được");
      log(f, "info", "Bạn góp ý plan: phân tích lại", "analyze");
      goTo(f, "analyze");
      startRun(f);
      break;
    case "selfCode":
      if (f.gate !== "planReview") throw fail("flow.notNow", "Lúc này chưa chọn tự code được");
      f.states.analyze = "done";
      f.states.implement = "waitingUser";
      toGate(f, "selfCode", "implement");
      log(f, "info", "Bạn tự code; xong thì bấm Tôi code xong", "implement");
      break;
    case "selfCodeDone":
      if (f.gate !== "selfCode") throw fail("flow.notNow", "Lúc này chưa được");
      f.states.implement = "done";
      goTo(f, "reviewLocal");
      startRun(f);
      break;
    case "finishTriage": {
      const open = f.findings.filter((x) => !settled(x) && !x.chosen && x.severity === "medium").length;
      if (open > 0) throw fail("triage.open", `Còn ${open} lỗi medium chưa quyết (sửa, chấp nhận hoặc để sau)`);
      f.states.finalReview = "done";
      goTo(f, f.findings.some(toFix) ? "fix" : "pushSummary");
      startRun(f);
      break;
    }
    case "rerunPush":
      if (running) throw fail("step.running", "Bước này đang chạy");
      if (f.phase !== "awaitingPush") throw fail("flow.notNow", "Lúc này chưa viết lại được");
      goTo(f, "pushSummary");
      startRun(f);
      break;
    case "finish":
      if (f.phase !== "awaitingPush") throw fail("flow.notNow", "Lúc này chưa đánh dấu xong được");
      f.phase = "done";
      log(f, "ok", "Bạn đã push: task xong");
      break;
    case "setAuto":
      f.auto = action.on;
      break;
  }
  return mockTaskDetail(wpId);
}

export function mockDecideFinding(d: FindingDecision): TaskDetail {
  const f = flowOf(d.wpId);
  const x = f.findings.find((o) => o.fingerprint === d.fingerprint);
  if (!x) throw fail("finding.notFound", "Không thấy lỗi này nữa");
  const reason = d.reason?.trim();
  switch (d.decision) {
    case "fix":
      x.chosen = true;
      x.reason = undefined;
      if (settled(x)) x.state = "open";
      break;
    case "accept":
      if (!reason) throw fail("finding.reason", "Chấp nhận thì ghi lý do (vì sao giữ code như vậy)");
      x.state = "accepted";
      x.chosen = false;
      x.reason = reason;
      break;
    case "defer":
      x.state = "deferred";
      x.chosen = false;
      x.reason = reason || undefined;
      break;
    case "reset":
      x.state = "open";
      x.chosen = false;
      x.reason = undefined;
      break;
  }
  return mockTaskDetail(d.wpId);
}

export function mockSaveBrief(wpId: number, text: string): TaskDetail {
  if (!text.trim()) throw fail("brief.empty", "Brief trống");
  flowOf(wpId).brief = text;
  return mockTaskDetail(wpId);
}

/** Card fields of a task with a mock flow (undefined when the task has none). */
export function mockFlowCard(wpId: number): Partial<TaskCard> | undefined {
  const f = flows.get(wpId);
  if (!f) return undefined;
  tick(f);
  const kind = f.current ?? "intake";
  const state: StepState =
    f.phase === "running" ? "running" : f.phase === "ready" ? "pending" : f.phase === "gate" ? (f.gate === "error" ? "failed" : "waitingUser") : "done";
  const cost = Object.values(f.costs).reduce((a, b) => a + (b ?? 0), 0);
  const card: Partial<TaskCard> = {
    column: f.phase === "awaitingPush" || f.phase === "done" ? "awaitingPush" : "active",
    step: { kind, index: STEP_ORDER.indexOf(kind) + 1, state, since: f.runStarted ?? f.gateSince },
    costUsd: cost > 0 ? cost : undefined,
    gate: f.phase === "gate" ? f.gate : undefined,
    flowNote: gateView(f)?.message,
    login: f.phase === "gate" && f.gate === "error" ? f.error?.provider : undefined,
  };
  if (f.phase === "running") {
    card.agent = agentOf(kind);
    card.contextPct = 18;
  }
  return card;
}

/** Map fields (steps, ship, belt, gate) of a task with a mock flow. */
export function mockFlowSystem(wpId: number): { steps: StepView[]; agents: AgentView[]; findings: FindingView[]; gate?: GateView } | undefined {
  const f = flows.get(wpId);
  if (!f) return undefined;
  tick(f);
  const agents: AgentView[] = [];
  if (f.current && f.phase !== "awaitingPush" && f.phase !== "done") {
    const t = tools(f);
    agents.push({
      id: `flow-${wpId}`,
      kind: agentOf(f.current),
      status: f.phase === "running" ? "running" : f.phase === "gate" ? "waiting" : "idle",
      step: f.current,
      contextTokens: f.phase === "running" ? 182_000 : 0,
      contextWindow: 1_000_000,
      handoffAt: 500_000,
      compactions: 0,
      subagents: [],
      children: [],
      recentTools: t,
    });
  }
  return { steps: steps(f), agents, findings: f.findings.map((x) => ({ ...x })), gate: gateView(f) };
}

/** Tests: forget every mock flow. */
export function resetMockFlows(): void {
  flows.clear();
}
