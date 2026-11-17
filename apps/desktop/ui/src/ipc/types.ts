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

export type GateKind = "planReview" | "mediumTriage" | "agentQuestion" | "error" | "selfCode";

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
  /** 1..8 */
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
  /** what the task waits for, when it waits for the user */
  gate?: GateKind;
  /** one line about the workflow: the error of a failed step, what the gate needs */
  flowNote?: string;
  /** the failed step needs this provider signed in again */
  login?: Provider;
  /** how the last "Lấy issue" went */
  issues?: IssueStatus;
  /** "Đã xong" column: when the user marked it done */
  finishedAt?: string;
  /** "Đã xong" column: the push check made when it was marked done */
  pushCheck?: PushCheck;
  /** "Đã xong" column: OpenProject still lists the task as open (not Resolved yet) */
  nxOpen?: boolean;
  /** the task's priority on OpenProject; absent when unknown */
  priority?: PriorityView;
}

/** The task's priority as OpenProject shows it: its name in the admin's colour. */
export interface PriorityView {
  /** as OpenProject shows it, e.g. "High" */
  name: string;
  /** `#rrggbb` when the OpenProject admin gave the priority a colour */
  color?: string;
}

/** Is the task branch on origin? Made by "Đã push xong" (`git fetch`, then the local branch
 *  against `origin/<branch>`). Read only: the app never pushes. */
export interface PushCheck {
  branch: string;
  /** origin has the branch and every local commit of it */
  pushed: boolean;
  /** origin has the branch at all */
  remote: boolean;
  /** local commits origin does not have */
  ahead: number;
  /** commits on origin the local branch does not have */
  behind: number;
  /** files changed but not committed (counted only while the repo is on the task branch) */
  uncommitted: number;
  /** short hash of the local branch */
  head?: string;
  /** `git fetch` failed: the result compares with what origin had at the last fetch */
  fetchError?: string;
  /** the repo is on another branch right now */
  otherBranch?: string;
  checkedAt: string;
  /** the user marked the task done although the check said it was not pushed */
  forced?: boolean;
}

/** The last "Lấy issue" of a task: when, how many issues reached the brief, or what failed. */
export interface IssueStatus {
  at: string;
  read: number;
  error?: string;
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
  /** alerts in the bell the user has not opened yet */
  unreadAlerts?: number;
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
  /** which Windows notifications are on */
  notify?: NotifySettings;
  /** agent, model and effort of each AI step, with past cost and time */
  steps?: StepSetting[];
}

/** Models and effort levels the Claude / Codex CLIs on this machine offer (`step_models`). */
export interface StepModels {
  claude: AgentCatalog;
  codex: AgentCatalog;
}

/** What one agent CLI offers; `fromCli: false` = the app's built-in list, `error` says why. */
export interface AgentCatalog {
  agent: AgentKind;
  models: AgentModel[];
  /** model the CLI uses without a flag, as configured (e.g. "opus[1m]") */
  defaultModel?: string;
  /** entry of `models` that `defaultModel` resolves to */
  defaultId?: string;
  /** "Opus 5.5 · 1M" */
  defaultLabel?: string;
  /** effort the CLI uses without a flag */
  defaultEffort?: string;
  fromCli: boolean;
  cliVersion?: string;
  error?: string;
  /** RFC 3339 */
  readAt?: string;
}

export interface AgentModel {
  /** what goes into --model / -m */
  id: string;
  label: string;
  description?: string;
  /** effort levels this model takes; empty = no effort flag */
  efforts: string[];
  defaultEffort?: string;
  isDefault: boolean;
}

/** One AI step in Settings → Các bước AI; absent model/effort = the CLI's default. */
export interface StepSetting {
  step: StepKind;
  agent: AgentKind;
  /** the agent of this step can be switched (cross-review only for now) */
  agentChoice: boolean;
  model?: string;
  effort?: string;
  /** effort the app passes when none is picked (push summary: "low") */
  effortDefault?: string;
  /** finished runs of this step over every task */
  runs: number;
  avgCostUsd?: number;
  avgSeconds?: number;
  /** model of the latest finished run */
  lastModel?: string;
}

/** What the user picked for one step; absent = the machine's default. */
export interface StepChoiceRequest {
  step: StepKind;
  agent?: AgentKind;
  model?: string;
  effort?: string;
}

/** Windows notifications by kind; the bell lists every alert either way. */
export interface NotifySettings {
  /** new OpenProject comments, task moved to Test failed / Reopened */
  comments: boolean;
  /** a step finished, waits for you or failed */
  steps: boolean;
  /** a quota window passed 80% / 95% */
  quota: boolean;
}

export type NotifyKind = keyof NotifySettings;

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
  /** comments newer than the previous read (0 on the first read) */
  newComments?: number;
}

// ---------- comments ----------

export interface CommentView {
  /** OpenProject activity id, or `<issue url>#<n>` */
  id: string;
  createdAt: string;
  /** OpenProject only; issue comments are read without names */
  author?: string;
  text: string;
  /** the user wrote it (OpenProject: author = users/me; GitHub: viewerDidAuthor): shown folded, never new */
  mine: boolean;
  /** someone else wrote it after the user last opened the Comment tab ("cần xem") */
  isNew: boolean;
  /** the comment on OpenProject, or the GitHub issue; open with openTaskLink */
  url?: string;
}

export interface IssueThread {
  url: string;
  title: string;
  /** newest first */
  comments: CommentView[];
  /** comments GitHub counts that the page did not give */
  unread: number;
}

export interface CommentsView {
  /** OpenProject comments, newest first */
  comments: CommentView[];
  /** why OpenProject comments could not be read */
  error?: string;
  taskUrl?: string;
  /** issues of the last "Lấy issue" */
  issues: IssueThread[];
  issuesReadAt?: string;
  /** OpenProject has comments newer than the brief's comment section */
  briefStale: boolean;
  fetchedAt: string;
}

// ---------- diff ----------

export type DiffChange = "added" | "deleted" | "modified" | "renamed";
export type DiffLineKind = "context" | "added" | "removed";

export interface DiffLine {
  kind: DiffLineKind;
  /** line number on the base side */
  old?: number;
  /** line number in the working tree */
  new?: number;
  text: string;
}

export interface DiffHunk {
  /** function or class git names after @@ */
  section: string;
  lines: DiffLine[];
}

export interface DiffFile {
  /** repo-relative, with / */
  path: string;
  oldPath?: string;
  change: DiffChange;
  binary: boolean;
  /** not added to git yet */
  untracked: boolean;
  added: number;
  removed: number;
  /** lines were left out (very long) */
  truncated: boolean;
  /** changed since the user last opened the Diff tab */
  changed: boolean;
  hunks: DiffHunk[];
}

export interface DiffView {
  base: string;
  /** short hash */
  forkPoint: string;
  files: DiffFile[];
  added: number;
  removed: number;
  /** files changed since the last look (0 the first time) */
  changedSinceSeen: number;
  seenBefore: boolean;
  /** e.g. the repo is on another branch */
  note?: string;
  truncated: boolean;
}

// ---------- alerts ----------

export type AlertKind = "comment" | "status" | "gate" | "stepDone" | "error" | "quota";

export interface AlertView {
  id: number;
  at: string;
  kind: AlertKind;
  wpId?: number;
  taskId?: string;
  title: string;
  body: string;
  /** task window tab that shows it */
  tab?: string;
  read: boolean;
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
  /** model and effort of the latest run (effort absent = the CLI's default) */
  model?: string;
  effort?: string;
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
  detail?: string;
  suggestion?: string;
  /** why the user accepted (or deferred) it */
  reason?: string;
  /** the user asked for it to be fixed */
  chosen?: boolean;
  /** cross-review round that reported it last */
  round?: number;
  /** step that reported it last */
  step?: StepKind;
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

// ---------- workflow (steps 2-8) ----------

/** ready = the current step runs when the user presses Run; gate = waits for the user. */
export type FlowStatus = "notStarted" | "ready" | "running" | "gate" | "awaitingPush" | "done";

export interface FlowError {
  /** agent.missing, agent.signedOut, step.failed, step.stopped, step.timeout, step.budget, git.unexpected, … */
  code: string;
  message: string;
  /** signing in to this provider may help */
  provider?: Provider;
}

export interface RunView {
  step: StepKind;
  agent: AgentKind;
  startedAt: string;
  /** newest last */
  tools: ToolEvent[];
  contextTokens?: number;
  /** model passed to the CLI, or the one it reported */
  model?: string;
  effort?: string;
  /** what runs: /review-local of the repo or the app's own prompt */
  command?: string;
}

/** What a review said about updating docs and tests. */
export interface ReviewNote {
  step: StepKind;
  text: string;
}

export interface PushBlock {
  label: string;
  text: string;
}

export interface PushView {
  issue?: number;
  /** whole answer, markdown */
  text: string;
  blocks: PushBlock[];
}

export interface LogLine {
  at: string;
  step?: StepKind;
  kind: "info" | "tool" | "ok" | "error";
  text: string;
}

export type RunState = "running" | "done" | "failed";

/** One run of a step, for the Tiến trình tab (oldest first). */
export interface RunSummary {
  id: number;
  step: StepKind;
  agent: AgentKind;
  state: RunState;
  startedAt: string;
  endedAt?: string;
  costUsd?: number;
  tokensIn?: number;
  tokensOut?: number;
  model?: string;
  effort?: string;
  /** cross-review round */
  round?: number;
  /** what the run produced in one line ("báo 3 lỗi (1 high, 2 medium)"), or why it failed */
  note?: string;
  /** tool calls the CLI refused (not on the allow list) */
  denials: DeniedCall[];
  /** the raw output is still on disk (the Chat tab can show it) */
  hasChat: boolean;
  /** command that continues the session in a terminal (`claude --resume <id>`) */
  resume?: string;
}

/** A tool call the CLI refused, with the allow-list entries that would let it run. */
export interface DeniedCall {
  /** "Bash", "Edit", … */
  tool: string;
  /** the whole command (or the path) */
  input: string;
  /** several commands joined by &&, ; or |: every part has to be allowed */
  chained: boolean;
  /** "npx tsc *", … (empty when it is not a shell command) */
  rules: RuleOption[];
}

export interface RuleOption {
  rule: string;
  /** already in the repo's allowBash */
  allowed: boolean;
}

export type ChatKind = "prompt" | "text" | "thinking" | "tool" | "end" | "error";

/** One item of the Chat tab. */
export interface ChatItem {
  kind: ChatKind;
  /** the prompt, the message, the tool's input (command, path) or the error */
  text: string;
  tool?: string;
  /** what the tool returned (cut) */
  output?: string;
  /** characters of the tool result left out */
  outputCut?: number;
  failed?: boolean;
  /** why the CLI refused the call */
  denied?: string;
}

/** The Chat tab: the transcript of one step run. */
export interface ChatView {
  run: RunSummary;
  items: ChatItem[];
  /** older items left out (very long runs) */
  dropped: number;
  /** the raw output is gone (logs are kept 14 days) */
  missing: boolean;
}

/** Everything the task window shows. */
export interface TaskDetail {
  wpId: number;
  taskId: string;
  title: string;
  project: string;
  repo?: string;
  branch?: string;
  base?: string;
  status: FlowStatus;
  /** step that runs next, runs now, failed, or waits for the user */
  current?: StepKind;
  steps: StepView[];
  gate?: GateView;
  error?: FlowError;
  running?: RunView;
  round: number;
  maxRounds: number;
  /** run the next step by itself (off: stop after each step) */
  autoAdvance: boolean;
  brief?: string;
  plan?: string;
  findings: FindingView[];
  /** docs/test judgement of the latest review of each kind */
  reviewNotes?: ReviewNote[];
  push?: PushView;
  /** newest last */
  log: LogLine[];
  /** every run of every step, oldest first (the Tiến trình tab) */
  runs: RunSummary[];
  costUsd: number;
  /** comments not seen in the Comment tab yet (OpenProject + last issue read) */
  newComments?: number;
}

export type FlowAction =
  | { type: "run" }
  | { type: "stop" }
  | { type: "retry" }
  | { type: "approvePlan" }
  | { type: "rejectPlan"; feedback: string }
  | { type: "selfCode" }
  | { type: "selfCodeDone" }
  | { type: "finishTriage" }
  | { type: "rerunPush"; issue?: number }
  /** the task moves to "Đã xong"; also before the push summary (the steps left are skipped).
   *  `forced`: the push check said "not pushed" and the user marks it done anyway */
  | { type: "finish"; forced?: boolean }
  /** "Làm tiếp": `step` = run again from there (absent = carry on where it stopped); `note`
   *  goes into the next Viết code */
  | { type: "reopen"; step?: StepKind; note?: string }
  | { type: "setAuto"; on: boolean };

export type Decision = "fix" | "accept" | "defer" | "reset";

export interface FindingDecision {
  wpId: number;
  fingerprint: string;
  decision: Decision;
  reason?: string;
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
  task_detail: TaskDetail;
  flow_action: TaskDetail;
  decide_finding: TaskDetail;
  save_brief: TaskDetail;
  open_task_folder: null;
  open_task_link: null;
  task_comments: CommentsView;
  refresh_brief_comments: CommentsView;
  task_diff: DiffView;
  alerts: AlertView[];
  mark_alerts_read: null;
  set_notify: SettingsView;
  set_step_choice: SettingsView;
  step_models: StepModels;
  step_chat: ChatView;
  allow_bash: TaskDetail;
  open_resume: null;
  push_check: PushCheck;
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
  task_detail: { wpId: number };
  flow_action: { wpId: number; action: FlowAction };
  decide_finding: { decision: FindingDecision };
  save_brief: { wpId: number; text: string };
  open_task_folder: { wpId: number };
  open_task_link: { wpId: number; url?: string };
  task_comments: { wpId: number; markSeen: boolean };
  refresh_brief_comments: { wpId: number };
  task_diff: { wpId: number; markSeen: boolean };
  mark_alerts_read: { ids: number[] };
  set_notify: { kind: NotifyKind; on: boolean };
  set_step_choice: { choice: StepChoiceRequest };
  step_models: { refresh: boolean };
  step_chat: { wpId: number; runId?: number };
  allow_bash: { wpId: number; rule: string };
  open_resume: { wpId: number; runId: number };
  push_check: { wpId: number };
}
