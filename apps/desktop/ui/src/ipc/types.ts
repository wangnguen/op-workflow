// IPC contract between the Rust core and the UI.
// Mirrors `opw_domain::view` (Rust, serde camelCase). Keep both sides in sync;
// a later phase generates this file from Rust (ts-rs/specta).
// Timestamps are ISO 8601 strings. Optional fields are omitted when absent.

export type AgentKind = "claude" | "codex" | "shell";

export type StepKind =
  | "intake"
  | "analyze"
  | "implement"
  | "checks"
  | "harness"
  | "reviewLocal"
  | "fix"
  | "crossReview"
  | "fixCross"
  | "browserTest"
  | "finalReview"
  | "pushSummary";

export type StepState = "pending" | "running" | "waitingUser" | "done" | "failed" | "skipped";

export type Severity = "critical" | "high" | "medium" | "low";

export type FindingState = "open" | "fixing" | "fixed" | "accepted" | "deferred";

export type SessionStatus = "running" | "waiting" | "idle";

export type TaskColumn = "inbox" | "active" | "awaitingPush" | "done";

/** `findings`: a review reported findings, the user picks what the AI fixes. `afterFix`: a Sửa
 *  lỗi finished, "Review chéo lại?". */
export type GateKind = "planReview" | "findings" | "afterFix" | "agentQuestion" | "error" | "selfCode";

/** The answer at the after-fix gate: Codex reviews again, go on (the final review; again after
 *  its own fix), or pick more findings of the review that was fixed. */
/** `browserTest`: after a fix of Review chốt's findings, test in the browser again, then Review chốt again. */
export type AfterFixChoice = "crossReview" | "next" | "pickMore" | "browserTest";

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
  /** place in OpenProject's order of priorities, 1 = lowest, out of `ranks` */
  rank?: number;
  ranks?: number;
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
/** What closing the main window does: ask, go to the tray (the AI keeps running) or quit (every
 *  running AI step is stopped with what it started). */
export type CloseChoice = "ask" | "tray" | "quit";

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
  verifyMcp?: boolean;
  /** probe Claude this often (seconds) while Claude Code is in use outside the app; 0 = off */
  claudeActiveProbeSec?: number;
  /** probe Claude this often (minutes) when Claude Code is not in use */
  claudeProbeMin?: number;
  /** what the window's X does */
  onClose?: CloseChoice;
  /** Claude sessions opened outside the app stay off the Phiên and Workspace screens */
  hideOutsideSessions?: boolean;
  /** harness library of every known repo */
  harnessLibs?: HarnessLib[];
  /** Kiểm tra commands of every known repo (empty = guessed per package) */
  repoChecks?: RepoChecks[];
}

/** Kiểm tra commands of one repo (Settings → Repo). */
export interface RepoChecks {
  /** the repo as written in config */
  repo: string;
  commands: string[];
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
  /** permission modes the CLI's --help lists (Claude --permission-mode, Codex --sandbox + flags) */
  modes: string[];
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
  maxBudgetUsd?: number;
  timeoutMin?: number;
  defaultBudgetUsd?: number;
  defaultTimeoutMin?: number;
  step: StepKind;
  agent: AgentKind;
  /** the agent of this step can be switched (every AI step but Test trình duyệt) */
  agentChoice: boolean;
  /** the agent used when none is picked (Review chéo: the other AI than Viết code's) */
  agentDefault: AgentKind;
  /** permission mode picked; absent = `permissionDefault` */
  permissionMode?: string;
  /** what the app passes when none is picked ("acceptEdits", "read-only"); absent = no flag */
  permissionDefault?: string;
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
  permissionMode?: string;
}

/** Windows notifications by kind; the bell lists every alert either way. */
export interface NotifySettings {
  /** new OpenProject comments, task moved to Test failed / Reopened */
  comments: boolean;
  /** a step finished, waits for you or failed */
  steps: boolean;
  /** a quota window passed 80% / 95% */
  quota: boolean;
  /** a Workspace session waits for you while you look elsewhere */
  sessions: boolean;
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

/** Harness library of one repo (Settings → Repo & nhánh). */
export interface HarnessLib {
  /** the repo as written in config */
  repo: string;
  dir: string;
  /** set by the user (otherwise `C:\tmp\<repo>-harness`) */
  custom: boolean;
  /** made the first time a task of the repo uses Harness */
  exists: boolean;
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
  /** repos those links name (host/owner/repo) */
  issueRepos?: string[];
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
  /** starting again a task that finished a round: where to carry on (none = where it stopped) */
  carryOn?: CarryOn;
}

/** "Làm tiếp": run again from a step the task reached (a note goes to Viết code). */
export interface CarryOn {
  step?: StepKind;
  note?: string;
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
  /** with `error`: `comments` is the last read, saved at this time */
  savedAt?: string;
  taskUrl?: string;
  /** issues of the last "Lấy issue" */
  issues: IssueThread[];
  issuesReadAt?: string;
  /** OpenProject has comments newer than the brief's comment section */
  briefStale: boolean;
  fetchedAt: string;
}

/** What a task asks: the Inbox "Xem task" box and the Workspace Brief tab. */
export interface TaskText {
  /** OpenProject's description as written there (markdown); "" when it has none */
  description: string;
  /** why the description could not be read now: `description` is the last task list's copy */
  descriptionError?: string;
  /** brief.md of a started task (what the AI reads) */
  brief?: string;
  taskUrl?: string;
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
  /** Moved to Deleted; removed permanently 30 days after this time. */
  deletedAt?: string;
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
  fixHarness?: boolean;
  /** cross-review round that reported it last */
  round?: number;
  /** step that reported it last */
  step?: StepKind;
}

export interface GateView {
  kind: GateKind;
  since: string;
  message: string;
  /** findings / after-fix gates: the review whose findings they are about */
  review?: StepKind;
  /** where "go on" leads (past the findings without fixing, or "No" after a fix) */
  next?: StepKind;
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
  /** OpenProject work package id (opens the task there) */
  wpId: number;
  title: string;
  project: string;
  nxStatus: string;
  nxStatusColor?: string;
  priority?: PriorityView;
}

/** A Claude session opened outside the app (VS Code, a terminal), as a far star. */
export interface DistantSession {
  id: string;
  agent: AgentKind;
  /** short name under the star: its folder */
  label: string;
  status: SessionStatus;
  /** what it is about (Claude's title or its first message) */
  title?: string;
  /** the folder it runs in */
  cwd?: string;
  /** where it is open: VS Code, terminal, … */
  openIn?: string;
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
  /** `quota.hit`: when the usage limit resets */
  resetsAt?: string;
}

/** The step about to run costs tokens and a quota window passed `quota.warnAt`. */
export interface QuotaNote {
  step: StepKind;
  provider: Provider;
  window: "fiveHour" | "sevenDay";
  usedPct: number;
  resetsAt?: string;
  /** at or over the highest level: "Tự chạy" stops here */
  stop: boolean;
  /** what this step cost the last time it ran (any task) */
  lastCostUsd?: number;
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

/** `waiting` = the AI stopped to ask the user; the step is not over. */
export type RunState = "running" | "done" | "failed" | "waiting";

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

export type ChatKind =
  | "prompt"
  | "user"
  | "text"
  | "thinking"
  | "tool"
  | "end"
  | "error"
  /** a line from the app in a session's chat (not sent to the AI) */
  | "note";

/** One item of the Chat tab. */
export interface ChatItem {
  step?: StepKind;
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

/** The analysis's advice for an optional step of this task. */
export interface StepProposal {
  step: StepKind;
  recommend: boolean;
  reason: string;
  /** what a harness would check, or the browser test scenario */
  checks?: string;
  /** browser test: how the analysis thinks the repo runs locally (only a proposal) */
  devServer?: DevServer;
}

/** How Test trình duyệt reaches the app: the URL Chrome opens and the commands that serve it.
 *  No command = the app is already running there. */
export interface DevServer {
  url: string;
  /** Run top to bottom (one `pnpm dev`, or an API then the web…); none = already running. */
  commands: DevCommand[];
  /** "Cách vào chỗ test": how to reach the screen to test, in the user's words (the browser
   *  test's prompt carries it); kept even without a URL */
  howTo?: string;
}

/** One command: from `dir` (inside the repo, "" = its root), ready once `wait` answers (a port
 *  on localhost or a URL; "" = the dev server's URL). Already answering = left as it is. */
export interface DevCommand {
  command: string;
  dir: string;
  wait: string;
}

/** What the AI of a step stopped to ask (question gate). */
export interface AgentQuestionView {
  step: StepKind;
  question: string;
  /** the page the AI was on */
  url?: string;
  /** the test's Chrome window is still open */
  browserOpen: boolean;
}

/** Everything the task window shows. */
export interface TaskDetail {
  wpId: number;
  taskId: string;
  title: string;
  project: string;
  /** back in the Inbox: the history can be read, steps wait for "Bắt đầu làm việc" */
  parked?: boolean;
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
  /** cross-review rounds so far (no cap) */
  round: number;
  /** "Tự chạy": run the next step by itself, stopping at every decision or error (off: stop
   *  after each step) */
  autoAdvance: boolean;
  brief?: string;
  plan?: string;
  /** the analysis's advice on Harness and Test trình duyệt */
  proposals: StepProposal[];
  /** optional steps this task runs (Harness, Test trình duyệt) */
  uses: StepKind[];
  /** Test trình duyệt: the dev server the app starts (this task's, else the repo's) */
  devServer?: DevServer;
  /** `devServer` is the one remembered for the repo */
  devServerFromRepo?: boolean;
  /** at the question gate: what the AI asks */
  agentQuestion?: AgentQuestionView;
  /** stopped with Dừng, and the run's session can go on: "Chạy tiếp" with the user's words */
  canGoOn?: boolean;
  continuationBudgetUsd?: number;
  /** "Trao đổi với AI" at the error gate */
  talk?: TalkTurn[];
  /** the AI is answering the last message of `talk` */
  talkBusy?: boolean;
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
  /** the last result of Harness and Test trình duyệt, when they ran */
  tests?: TestView[];
  /** the next step costs tokens and the quota is high */
  quotaNote?: QuotaNote;
}

/** What Harness (step 5) or Test trình duyệt (step 11) found the last time it ran. */
export interface TestView {
  step: StepKind;
  passed: boolean;
  summary: string;
  /** failed cases first */
  cases: TestCase[];
  at: string;
  /** runs so far (the AI's build and the app's runs after each fix) */
  runs: number;
  /** harness: the command the app runs again after each fix, and its folder */
  command?: string;
  dir?: string;
  /** harness: the broken copy made it FAIL (absent = the AI did not say) */
  negative?: boolean;
  negativeDetail?: string;
  /** harness: the folder it was kept in, in the repo's harness library */
  library?: string;
  /** browser test: the page the AI opened */
  url?: string;
  /** browser test: real data the AI changed while testing (where; undone or not) */
  changes?: string[];
}

export interface TestCase {
  name: string;
  ok: boolean;
  detail?: string;
  /** browser test: the screenshot of the case */
  screenshot?: string;
  /** browser test: the AI could not run it (`detail` says why); not an NG */
  untested?: boolean;
}

/** One message of "Trao đổi với AI". */
export interface TalkTurn {
  fromUser: boolean;
  text: string;
  at: string;
  costUsd?: number;
}

export type FlowAction =
  | { type: "talk"; text: string }
  | { type: "run" }
  | { type: "stop" }
  | { type: "retry" }
  /** after Dừng: go on in the stopped run's session, `note` (may be empty) as its next message */
  | { type: "goOn"; note: string }
  | { type: "approvePlan" }
  | { type: "rejectPlan"; feedback: string }
  | { type: "selfCode" }
  /** turn Harness / Test trình duyệt on or off for this task, before the flow gets there */
  | { type: "setUses"; step: StepKind; on: boolean }
  | { type: "setWorkspaceFlow"; on: boolean }
  | { type: "setWorkspaceStep"; step: StepKind; on: boolean }
  | { type: "runWorkspaceStep"; step: StepKind }
  /** Workspace: start the task again at Phân tích, in a new session */
  | { type: "startOver" }
  | { type: "selfCodeDone" }
  /** Test trình duyệt: the dev server the app starts (remembered for the repo); empty url = none */
  | { type: "setDevServer"; url: string; commands: DevCommand[]; howTo?: string }
  /** question gate: the user did what the AI asked (or answers it); the step goes on */
  | { type: "answerAgent"; answer: string }
  /** findings gate: fix the picked findings, or go on without fixing */
  | { type: "pickFindings"; fix: boolean }
  /** after-fix gate: the answer to "Review chéo lại?" */
  | { type: "afterFix"; choice: AfterFixChoice }
  /** take the harness this task kept out of the repo's harness library */
  | { type: "dropHarness" }
  | { type: "rerunPush"; issue?: number }
  /** the task moves to "Đã xong"; also before the push summary (the steps left are skipped).
   *  `forced`: the push check said "not pushed" and the user marks it done anyway */
  | { type: "finish"; forced?: boolean }
  /** "Làm tiếp": `step` = run again from there (absent = carry on where it stopped); `note`
   *  goes into the next Viết code */
  | { type: "reopen"; step?: StepKind; note?: string }
  | { type: "setAuto"; on: boolean };

export type Decision = "fix" | "fixHarness" | "accept" | "defer" | "reset";

export interface FindingDecision {
  wpId: number;
  fingerprint: string;
  decision: Decision;
  reason?: string;
}

// ---------- Workspace sessions (PLAN.md §12 B) ----------

/** main = a task's session (the newest is the task's current one); free = in a repo, no task. */
export type SessionKind = "main" | "free";
/** idle: nothing runs (the next message starts or resumes it) · running: a turn runs ·
 *  waiting: it waits for you · outside: open in VS Code or a terminal, the app only shows it */
export type SessionState = "idle" | "running" | "waiting" | "outside";

export interface SessionView {
  /** Claude Code session id */
  id: string;
  kind: SessionKind;
  wpId?: number;
  taskId?: string;
  title: string;
  repo: string;
  branch?: string;
  state: SessionState;
  /** the app runs a process for it now */
  live: boolean;
  terminal?: boolean;
  shell?: Exclude<TerminalKind, "claude">;
  contextPercent?: number;
  /** where it is open while the app only shows it ("VS Code", "terminal") */
  outside?: string;
  /** permission mode in effect, else the one chosen for the next start */
  mode?: string;
  model?: string;
  costUsd: number;
  /** it has a transcript (a message was sent at least once) */
  started: boolean;
  createdAt: string;
  updatedAt: string;
  /** why the last start or run failed */
  error?: string;
}

export type TerminalKind = "claude" | "powerShell" | "gitBash";
export interface TerminalFrame {
  bytes: number[];
  replay: boolean;
  skipped: boolean;
}
export type TerminalAction =
  | { kind: "ack" | "detach" | "close" }
  | { kind: "write"; data: string }
  | { kind: "resize"; data: { cols: number; rows: number } };

/** A task in progress, in the tree even before it has a session. */
export interface WorkspaceTask {
  wpId: number;
  taskId: string;
  title: string;
  project: string;
  repo?: string;
  branch?: string;
}

/** An image pasted into the chat (base64). */
export interface ImageIn {
  mediaType: string;
  data: string;
}

/** A package.json script, run in the session's shell. */
export interface RepoScript {
  name: string;
  command: string;
  /** what to type: `pnpm run dev` */
  run: string;
}

/** A command `/` offers in the chat. */
export interface SlashCommand {
  name: string;
  description?: string;
  /** claude, repo, user, skill, plugin */
  source: string;
}

export interface WorkspaceFlow {
  /** the task's main session, where its steps go (the newest after Làm lại từ đầu) */
  session?: string;
  enabled: boolean;
  auto: boolean;
  running: StepKind | null;
  sessionStep: boolean;
  gate: GateKind | null;
  error: { code: string; message: string } | null;
  /** `whyNot`: why the step cannot run now (nothing to review yet, no finding picked) */
  steps: { step: StepKind; enabled: boolean; state: StepState; whyNot?: string }[];
}

export interface WorkspaceSnapshot {
  tasks: WorkspaceTask[];
  /** newest first */
  sessions: SessionView[];
  /** Claude sessions open outside the app (empty when hidden in Settings), newest first */
  outside?: OutsideSession[];
}

/** A Claude session opened outside the app (VS Code, a terminal); the app only shows it. */
export interface OutsideSession {
  id: string;
  /** the folder it runs in */
  cwd: string;
  title: string;
  /** idle, running or waiting */
  state: SessionState;
  /** where it is open: VS Code, terminal, … */
  openIn: string;
  /** when its status last changed */
  updatedAt?: string;
}

/** The conversation of a session opened outside the app, read from its transcript. */
export interface OutsideChat {
  session: OutsideSession;
  items: ChatItem[];
  /** older items left out */
  dropped: number;
}

/** permission: a tool call needs approval · question: AskUserQuestion · plan: ExitPlanMode ·
 *  trust: Claude Code has no trust record for the folder yet */
export type AskKind = "permission" | "question" | "plan" | "trust";

export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  question: string;
  header?: string;
  options: AskOption[];
  multiSelect: boolean;
}

/** Something the session waits for you to answer. */
export interface SessionAsk {
  id: string;
  kind: AskKind;
  /** tool name; empty for trust */
  tool: string;
  /** the command, path or folder */
  summary: string;
  detail?: string;
  /** the rule "always allow" would add, in words; absent = not offered */
  always?: string;
  questions?: AskQuestion[];
  /** the plan to approve (markdown) */
  plan?: string;
}

export type AskAnswer =
  | { kind: "allow"; always?: boolean }
  | { kind: "deny"; message?: string }
  | { kind: "answers"; answers: Record<string, string> }
  | { kind: "approvePlan"; mode?: string }
  | { kind: "rejectPlan"; feedback?: string }
  | { kind: "trust"; trust: boolean };

export interface SessionChat {
  session: SessionView;
  items: ChatItem[];
  dropped: number;
  /** text the AI is writing right now */
  partial?: string;
  asks?: SessionAsk[];
  /** grows with every change */
  seq: number;
}

/** A change pushed while a session runs: `items` replace the chat's items from `from` on. */
export interface SessionPatch {
  id: string;
  seq: number;
  from: number;
  items: ChatItem[];
  dropped: number;
  partial?: string;
  asks?: SessionAsk[];
  session: SessionView;
}

export interface NewSession {
  wpId?: number;
  repo?: string;
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
  task_text: TaskText;
  refresh_brief_comments: CommentsView;
  task_diff: DiffView;
  alerts: AlertView[];
  mark_alerts_read: null;
  delete_alerts: AlertView[];
  restore_alerts: AlertView[];
  set_notify: SettingsView;
  set_step_choice: SettingsView;
  set_verify_mcp: SettingsView;
  set_step_limits: SettingsView;
  set_quota_probe: SettingsView;
  step_models: StepModels;
  step_chat: ChatView;
  allow_bash: TaskDetail;
  open_resume: null;
  push_check: PushCheck;
  close_window: null;
  set_on_close: SettingsView;
  set_harness_dir: SettingsView;
  set_repo_checks: SettingsView;
  open_task_path: null;
  workspace_snapshot: WorkspaceSnapshot;
  workspace_repos: string[];
  session_new: SessionView;
  session_chat: SessionChat;
  session_send: SessionView;
  session_answer: SessionView;
  session_interrupt: null;
  session_set_mode: SessionView;
  session_stop: SessionView;
  session_open_vscode: null;
  session_hide: null;
  session_diff: DiffView;
  session_open_file: null;
  task_open_file: null;
  session_compare_file: null;
  task_compare_file: null;
  session_open_repo: null;
  session_files: string[];
  session_commands: SlashCommand[];
  session_scripts: RepoScript[];
  session_notify_wait: null;
  outside_chat: OutsideChat;
  set_hide_outside_sessions: SettingsView;
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
  task_text: { wpId: number; readNow: boolean };
  refresh_brief_comments: { wpId: number };
  task_diff: { wpId: number; markSeen: boolean };
  mark_alerts_read: { ids: number[] };
  delete_alerts: { ids: number[] };
  restore_alerts: { ids: number[] };
  set_notify: { kind: NotifyKind; on: boolean };
  set_step_choice: { choice: StepChoiceRequest };
  set_verify_mcp: { on: boolean };
  set_step_limits: { limits: { step: StepKind; maxBudgetUsd?: number; timeoutMin?: number } };
  set_quota_probe: { activeSec: number; idleMin: number };
  step_models: { refresh: boolean };
  step_chat: { wpId: number; runId?: number };
  allow_bash: { wpId: number; rule: string };
  open_resume: { wpId: number; runId: number };
  push_check: { wpId: number };
  close_window: { choice: CloseChoice; remember: boolean };
  set_on_close: { choice: CloseChoice };
  set_harness_dir: { repo: string; dir?: string };
  set_repo_checks: { repo: string; commands: string[] };
  open_task_path: { wpId: number; path: string };
  session_new: { request: NewSession };
  session_chat: { id: string };
  session_send: { id: string; text: string; force?: boolean; images?: ImageIn[] };
  session_answer: { id: string; askId: string; answer: AskAnswer };
  session_interrupt: { id: string };
  session_set_mode: { id: string; mode: string };
  session_stop: { id: string };
  session_open_vscode: { id: string };
  session_hide: { id: string };
  session_diff: { id: string };
  session_open_file: { id: string; path: string; line?: number };
  task_open_file: { wpId: number; path: string; line?: number };
  session_compare_file: { id: string; rev: string; path: string; oldPath?: string };
  task_compare_file: { wpId: number; rev: string; path: string; oldPath?: string };
  session_open_repo: { id: string };
  session_files: { id: string; query: string };
  session_commands: { id: string };
  session_scripts: { id: string };
  session_notify_wait: { id: string };
  outside_chat: { id: string };
  set_hide_outside_sessions: { hide: boolean };
}
