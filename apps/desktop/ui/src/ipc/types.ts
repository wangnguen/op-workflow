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

export type Provider = "claude" | "codex";

/** Connection to one provider's quota source (Codex app-server, Claude probe). */
export type LinkStatus = "connecting" | "ok" | "error" | "missing" | "signedOut";

export interface ProviderLink {
  provider: Provider;
  status: LinkStatus;
  /** what went wrong and what the app does next (Vietnamese) */
  message?: string;
  since: string;
}

export interface QuotaSnapshot {
  provider: Provider;
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
  /** `#rrggbb` the OpenProject admin gave this status */
  nxStatusColor?: string;
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
  /** tasks in "Đang làm", most recent first; several can run at once */
  activeTaskIds: string[];
  quota: QuotaSnapshot[];
  /** quota connections per provider; absent in demo data */
  links?: ProviderLink[];
  today: TodayStats;
  /** "demo" until an OpenProject token is saved, then "live" */
  mode: DataMode;
  /** problem to show above the board, e.g. OpenProject unreachable */
  notice?: string;
  /** briefs are still in the hidden data folder: offer this Desktop folder once */
  tasksDirOffer?: string;
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
  /** folders scanned for repos */
  workspaces: string[];
  /** what "Bắt đầu làm việc" remembered per OpenProject project */
  projects: ProjectMemory[];
  configPath?: string;
  dataDir?: string;
  /** browser that allows "Lấy issue" to connect right now ("Chrome"); absent = not enabled */
  browserDebug?: string;
  /** folder of the task folders (`<task id>/brief.md`) in use now */
  tasksDir?: string;
  /** false until the user picked a folder (or kept the default) once */
  tasksDirChosen: boolean;
  /** the Desktop folder the app suggests */
  tasksDirSuggested?: string;
}

/** Result of moving the briefs to another folder. */
export interface TasksDirChange {
  path: string;
  /** task folders moved from the old place */
  moved: number;
  /** folders that stayed behind, and why */
  warnings: string[];
}

export interface ProjectMemory {
  name: string;
  repos: string[];
  branch?: string;
}

/** Error shape of failing commands (`{code, message}`). */
export interface ApiError {
  code: string;
  message: string;
}

// ---------- start work ----------

/** A git repo found in the workspace folders (or remembered for the project). */
export interface RepoOption {
  /** absolute path; sent back in `StartRequest.repo` */
  path: string;
  name: string;
  remote?: string;
  /** remote matches an issue link in the task description */
  suggested: boolean;
  /** used for this project before */
  remembered: boolean;
}

export type BranchSource = "saved" | "project" | "default";

export interface StartOptions {
  wpId: number;
  taskId: string;
  title: string;
  project: string;
  nxStatus: string;
  repos: RepoOption[];
  /** repo to pre-select */
  repo?: string;
  /** branch name to pre-fill */
  branch: string;
  /** only "default" may be replaced by the convention guessed from the picked repo */
  branchSource: BranchSource;
  issueLinks: string[];
  workspaces: string[];
}

export interface BranchOption {
  /** "development" or "origin/development" */
  name: string;
  remote: boolean;
  committedAt?: string;
  /** "ahead 2", "behind 14" */
  track?: string;
  /** main branch (main, development, the remote's default…): never offered for deletion */
  protected?: boolean;
}

/** Delete one branch from the branch list (the user picked it and confirmed). */
export interface DeleteBranchRequest {
  repo: string;
  /** "f/WP-1" or "origin/f/WP-1" */
  branch: string;
  /** delete origin/<branch> instead of the local branch */
  remote: boolean;
  /** local only: also drop commits that are not merged anywhere (git branch -D) */
  force?: boolean;
  wpId?: number;
}

export interface RepoState {
  path: string;
  name: string;
  /** newest commit first */
  branches: BranchOption[];
  current?: string;
  /** `git status --porcelain` lines (capped) */
  dirty: string[];
  dirtyCount: number;
  lastBase?: string;
  /** branch name following the repo's own convention */
  suggestedBranch?: string;
}

export interface StartRequest {
  wpId: number;
  repo: string;
  base: string;
  branch: string;
}

export interface StartResult {
  taskId: string;
  branch: string;
  /** false when the branch existed and the app only switched to it */
  created: boolean;
  briefPath?: string;
  warnings: string[];
  /** GitHub issue links in the task; the board then runs "Lấy issue" by itself */
  issueLinks?: number;
}

/** "Lấy issue": issues read through the user's Chrome into the brief. */
export interface IssueFetch {
  read: number;
  /** issues that could not be read (the others are in the brief) */
  warnings: string[];
}

// ---------- update check ----------

export interface UpdateView {
  current: string;
  enabled: boolean;
  latest?: string;
  /** latest is newer than current */
  available: boolean;
  url?: string;
  publishedAt?: string;
  checkedAt?: string;
  error?: string;
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
  nxStatusColor?: string;
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
  /** most recent task in "Đang làm", drawn in the centre by default */
  active?: TaskSystem;
  /** the other tasks in "Đang làm", most recent first */
  others?: TaskSystem[];
  inbox: InboxItem[];
  distant: DistantSession[];
  quota: QuotaSnapshot[];
  /** why the inbox may be empty: loading, OpenProject error, URL missing */
  notice?: string;
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
  open_config: null;
  start_options: StartOptions;
  repo_state: RepoState;
  fetch_repo: RepoState;
  delete_branch: RepoState;
  start_task: StartResult;
  pause_task: null;
  open_brief: null;
  update_status: UpdateView;
  check_update: UpdateView;
  open_release_page: null;
  reconnect_quota: null;
  open_login: null;
  reveal_path: null;
  fetch_issues: IssueFetch;
  open_browser_debug: null;
  set_tasks_dir: TasksDirChange;
}

export interface CommandArgs {
  save_nx_token: { baseUrl: string; token: string };
  start_options: { wpId: number };
  repo_state: { repo: string; wpId?: number };
  fetch_repo: { repo: string; wpId?: number };
  delete_branch: { request: DeleteBranchRequest };
  start_task: { request: StartRequest };
  pause_task: { wpId: number };
  open_brief: { wpId: number };
  reconnect_quota: { provider?: Provider };
  open_login: { provider: Provider };
  reveal_path: { path: string };
  fetch_issues: { wpId: number };
  set_tasks_dir: { dir?: string };
}
