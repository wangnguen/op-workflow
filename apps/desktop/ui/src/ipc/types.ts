// IPC contract between the Rust core and the UI.
// Mirrors `opw_domain::view` (Rust, serde camelCase). Keep both sides in sync;
// a later phase generates this file from Rust (ts-rs/specta).
// Timestamps are ISO 8601 strings. Optional fields are omitted when absent.

export type AgentKind = "claude" | "codex" | "shell";

export type StepKind =
  "intake" | "analyze" | "implement" | "reviewLocal" | "fix" | "crossReview" | "finalReview" | "pushSummary";

export type StepState = "pending" | "running" | "waitingUser" | "done" | "failed" | "skipped";

export type Severity = "critical" | "high" | "medium" | "low";

export type FindingState = "open" | "fixing" | "fixed" | "accepted" | "deferred";

export type SessionStatus = "running" | "waiting" | "idle";

export type TaskColumn = "inbox" | "active" | "awaitingPush" | "done";

export type GateKind = "planReview" | "mediumTriage" | "agentQuestion" | "error";

export interface AppInfo {
  version: string;
  core: string;
}

// ---------- quota ----------

export interface QuotaWindow {
  kind: "fiveHour" | "sevenDay";
  /** 0..100 */
  usedPct: number;
  resetsAt?: string;
}

export interface QuotaSnapshot {
  provider: "claude" | "codex";
  windows: QuotaWindow[];
  /** e.g. "rate_limit_event", "app-server", "statusline", "rollout" */
  source: string;
  observedAt: string;
  plan?: string;
}

// ---------- board ----------

export interface StepRef {
  kind: StepKind;
  /** 1..7 */
  index: number;
  state: StepState;
  since: string;
}

export interface TaskCard {
  /** Display id, e.g. "WP-187716" */
  id: string;
  /** OpenProject work package id */
  wpId: number;
  title: string;
  project: string;
  repo?: string;
  branch?: string;
  /** OpenProject status name, e.g. "Open", "In progress" */
  nxStatus: string;
  column: TaskColumn;
  step?: StepRef;
  agent?: AgentKind;
  costUsd?: number;
  /** 0..100, share of the handoff threshold */
  contextPct?: number;
  newComments: number;
}

export interface TodayStats {
  tokens: number;
  costUsd: number;
}

export type DataMode = "demo" | "live";

export interface BoardSnapshot {
  tasks: TaskCard[];
  activeTaskId?: string;
  quota: QuotaSnapshot[];
  today: TodayStats;
  /** "demo" until an OpenProject token is saved, then "live" */
  mode: DataMode;
  /** problem to show above the board, e.g. OpenProject unreachable */
  notice?: string;
}

// ---------- settings ----------

/** Settings dialog data; never contains the token itself. */
export interface SettingsView {
  nxBaseUrl: string;
  nxTokenSet: boolean;
  nxUser?: string;
  nxTaskCount?: number;
  nxLastSync?: string;
  nxError?: string;
  claudeQuotaAt?: string;
  codexQuotaAt?: string;
  quotaError?: string;
}

/** Error shape of failing commands (`{code, message}`). */
export interface ApiError {
  code: string;
  message: string;
}

// ---------- orbit ----------

export interface StepView {
  kind: StepKind;
  /** 1..7, also the orbit number counted from the star */
  index: number;
  state: StepState;
  startedAt?: string;
  endedAt?: string;
  costUsd?: number;
  /** review/fix round, starts at 1 */
  round?: number;
}

export interface SubagentView {
  name: string;
  status: "running" | "done";
}

export interface ToolEvent {
  at: string;
  name: string;
}

export interface AgentView {
  id: string;
  kind: AgentKind;
  status: SessionStatus;
  step: StepKind;
  contextTokens: number;
  contextWindow: number;
  /** token count where the app hands off and compacts */
  handoffAt: number;
  compactions: number;
  subagents: SubagentView[];
  /** e.g. Codex launched by Claude */
  children: AgentView[];
  recentTools: ToolEvent[];
}

export interface FindingView {
  fingerprint: string;
  severity: Severity;
  title: string;
  file: string;
  line?: number;
  /** "codex", "claude", "review-local" */
  source: string;
  state: FindingState;
}

export interface GateView {
  kind: GateKind;
  since: string;
  message: string;
}

export interface TaskSystem {
  taskId: string;
  title: string;
  repo: string;
  branch: string;
  nxStatus: string;
  startedAt: string;
  steps: StepView[];
  agents: AgentView[];
  findings: FindingView[];
  newComments: number;
  gate?: GateView;
}

export interface InboxItem {
  taskId: string;
  title: string;
  project: string;
}

export interface DistantSession {
  id: string;
  agent: AgentKind;
  label: string;
  status: SessionStatus;
}

export interface OrbitSnapshot {
  active?: TaskSystem;
  inbox: InboxItem[];
  distant: DistantSession[];
  quota: QuotaSnapshot[];
}

// ---------- commands ----------

/** Tauri command names → result type. Arguments are listed in `CommandArgs`. */
export interface Commands {
  app_info: AppInfo;
  board_snapshot: BoardSnapshot;
  orbit_snapshot: OrbitSnapshot;
  settings_status: SettingsView;
  save_nx_token: SettingsView;
  clear_nx_token: SettingsView;
  refresh_now: null;
}

export interface CommandArgs {
  save_nx_token: { baseUrl: string; token: string };
}
