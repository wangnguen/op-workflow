// The only module that talks to Tauri. Outside Tauri (plain browser) it serves mock data.
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  mockAiClis,
  mockAppInfo,
  mockBoard,
  mockClearToken,
  mockDeleteBranch,
  mockFetchIssues,
  mockFetchRepo,
  mockOrbit,
  mockReconnect,
  mockRepoState,
  mockSaveToken,
  mockSetTasksDir,
  mockSetHarnessDir,
  mockSetRepoChecks,
  mockSetOnClose,
  mockSetHideOutside,
  mockSetQuotaProbe,
  mockSetStepChoice,
  mockSetVerifyMcp,
  mockSetStepLimits,
  mockSettings,
  mockStepModels,
  mockStartOptions,
  mockStartTask,
  mockUpdate,
} from "./mock";
import {
  mockAllowBash,
  mockDecideFinding,
  mockFlowAction,
  mockPauseTask,
  mockPushCheck,
  mockSaveBrief,
  mockStepChat,
  mockTaskDetail,
  mockTaskText,
} from "./mockFlow";
import {
  mockAlerts,
  mockMarkAlertsRead,
  mockDeleteAlerts,
  mockRestoreAlerts,
  mockRefreshBriefComments,
  mockSetNotify,
  mockTaskComments,
  mockTaskDiff,
} from "./mockTask";
import {
  mockAdoptOutside,
  mockArchiveOutside,
  mockForkOutside,
  mockRestoreSession,
  mockSessionAnswer,
  mockSessionChat,
  mockSessionHide,
  mockSessionInterrupt,
  mockSessionNew,
  mockOutsideChat,
  mockSessionOpenVscode,
  mockSessionSend,
  mockSessionSetMode,
  mockSessionStop,
  mockWorkspace,
  mockWorkspaceRepos,
  onMockSessionPatch,
} from "./mockWorkspace";
import type {
  AiCli,
  AskAnswer,
  NewSession,
  OutsideChat,
  SessionChat,
  SessionPatch,
  SessionView,
  TerminalAction,
  TerminalFrame,
  TerminalKind,
  WorkspaceSnapshot,
  ImageIn,
  RepoScript,
  SlashCommand,
  WorkspaceFlow,
  StepKind,
  CloseChoice,
  AlertView,
  ApiError,
  AppInfo,
  BoardSnapshot,
  ChatView,
  CommentsView,
  DiffView,
  NotifyKind,
  CommandArgs,
  Commands,
  DeleteBranchRequest,
  FindingDecision,
  FlowAction,
  IssueFetch,
  OrbitSnapshot,
  Provider,
  PushCheck,
  RepoState,
  SettingsView,
  StartOptions,
  StartRequest,
  StartResult,
  StepChoiceRequest,
  StepModels,
  TaskDetail,
  TaskText,
  TasksDirChange,
  UpdateView,
} from "./types";

async function call<K extends keyof Commands>(
  command: K,
  mock: () => Commands[K],
  args?: K extends keyof CommandArgs ? CommandArgs[K] : never,
): Promise<Commands[K]> {
  if (isTauri()) return invoke<Commands[K]>(command, args);
  return mock();
}

export async function openTerminal(
  id: string, kind: TerminalKind, view: string, size: [number, number],
  onOutput: (frame: TerminalFrame) => void,
): Promise<void> {
  if (!isTauri()) {
    onOutput({
      bytes: [...new TextEncoder().encode("Terminal chỉ chạy trong ứng dụng desktop.\r\n")],
      replay: true,
      skipped: false,
    });
    return;
  }
  const output = new Channel<TerminalFrame>(onOutput);
  await invoke("terminal_open", { id, kind, view, size, output });
}

export async function controlTerminal(
  id: string, kind: TerminalKind, view: string, action: TerminalAction,
): Promise<void> {
  if (isTauri()) await invoke("terminal_control", { id, kind, view, action });
}

const previewFlows = new Map<number, WorkspaceFlow>();
const PREVIEW_NEEDS_CODE = "Chưa có code để kiểm hay review: chạy Viết code, hoặc nhờ Claude sửa trong chat trước.";
const PREVIEW_NEEDS_PICK =
  "Chưa có lỗi nào được chọn để sửa: chạy một bước review, rồi chọn lỗi cần sửa (Xem quyết định).";

/** The preview's rule for a step that cannot run yet, like the app's (code from Viết code on). */
function previewWhyNot(step: StepKind, coded: boolean): string | undefined {
  if (step === "analyze" || step === "implement" || step === "intake") return undefined;
  if (step === "fix" || step === "fixCross") return PREVIEW_NEEDS_PICK;
  return coded ? undefined : PREVIEW_NEEDS_CODE;
}

function previewFlow(wpId: number): WorkspaceFlow {
  const detail = mockTaskDetail(wpId);
  const steps = detail.steps.filter((s) => s.kind !== "intake");
  const coded = steps.some((s) => s.kind === "implement" && s.state === "done");
  return {
    enabled: false, auto: detail.autoAdvance, running: null, sessionStep: false,
    gate: null, error: null,
    steps: steps.map((s) => ({
      step: s.kind, enabled: !["harness", "browserTest"].includes(s.kind), state: s.state,
      whyNot: previewWhyNot(s.kind, coded),
    })),
  };
}

export async function getWorkspaceFlow(wpId: number): Promise<WorkspaceFlow> {
  if (isTauri()) return invoke("workspace_flow", { wpId });
  let view = previewFlows.get(wpId);
  if (!view) {
    view = previewFlow(wpId);
    previewFlows.set(wpId, view);
  }
  return view;
}

export async function workspaceFlowAction(wpId: number, action: FlowAction): Promise<WorkspaceFlow> {
  if (isTauri()) {
    await flowAction(wpId, action);
  } else {
    const view = await getWorkspaceFlow(wpId);
    if (action.type === "setWorkspaceFlow") previewFlows.set(wpId, { ...view, enabled: action.on });
    if (action.type === "setAuto") previewFlows.set(wpId, { ...view, auto: action.on });
    if (action.type === "startOver") {
      previewFlows.set(wpId, {
        ...view,
        steps: view.steps.map((s) => ({
          ...s, state: s.step === "analyze" ? "running" : "pending", whyNot: previewWhyNot(s.step, false),
        })),
      });
    }
    if (action.type === "runWorkspaceStep") {
      const why = view.steps.find((s) => s.step === action.step)?.whyNot;
      if (why) throw { code: "step.notReady", message: why };
    }
    if (action.type === "setWorkspaceStep" || action.type === "runWorkspaceStep") {
      previewFlows.set(wpId, {
        ...view,
        steps: view.steps.map((s) => s.step !== action.step ? s : action.type === "setWorkspaceStep"
          ? { ...s, enabled: action.on } : { ...s, state: "done" }),
      });
    }
  }
  return getWorkspaceFlow(wpId);
}

export async function completeWorkspaceStep(wpId: number, step: StepKind): Promise<void> {
  if (isTauri()) await invoke("workspace_step_done", { wpId, step });
}

export const getAppInfo = (): Promise<AppInfo> => call("app_info", mockAppInfo);
export const getBoard = (): Promise<BoardSnapshot> => call("board_snapshot", () => mockBoard(Date.now()));
export const getOrbit = (): Promise<OrbitSnapshot> => call("orbit_snapshot", () => mockOrbit(Date.now()));

export const getSettings = (): Promise<SettingsView> => call("settings_status", mockSettings);
/** Checks the token against OpenProject, then stores it in Windows Credential Manager. */
export const saveNxToken = (baseUrl: string, token: string): Promise<SettingsView> =>
  call("save_nx_token", () => mockSaveToken(baseUrl, token), { baseUrl, token });
export const clearNxToken = (): Promise<SettingsView> => call("clear_nx_token", mockClearToken);
export const refreshNow = (): Promise<null> => call("refresh_now", () => null);
/** Opens a folder (or shows a file) listed in Settings in Explorer. */
export const revealPath = (path: string): Promise<null> => call("reveal_path", () => null, { path });
/** Shows config.json in Explorer. */
export const openConfig = (): Promise<null> => call("open_config", () => null);
/** Settings → Các bước AI: agent, model and effort of one step (absent = the machine's default). */
export const setStepChoice = (choice: StepChoiceRequest): Promise<SettingsView> =>
  call("set_step_choice", () => mockSetStepChoice(choice), { choice });

export const setVerifyMcp = (on: boolean): Promise<SettingsView> =>
  call("set_verify_mcp", () => mockSetVerifyMcp(on), { on });
export const setStepLimits = (limits: CommandArgs["set_step_limits"]["limits"]): Promise<SettingsView> =>
  call("set_step_limits", () => mockSetStepLimits(limits), { limits });
/** Settings → Hạn mức: probe Claude every `activeSec` while Claude Code is in use elsewhere
 *  (0 = off), and every `idleMin` minutes otherwise. */
export const setQuotaProbe = (activeSec: number, idleMin: number): Promise<SettingsView> =>
  call("set_quota_probe", () => mockSetQuotaProbe(activeSec, idleMin), { activeSec, idleMin });
/** Models and effort levels the Claude / Codex CLIs on this machine offer. The core starts both
 *  CLIs (no prompt, no tokens, ~2 s) and reuses the answer for 10 minutes unless `refresh`. */
export async function getStepModels(refresh: boolean): Promise<StepModels> {
  const models = await call("step_models", mockStepModels, { refresh });
  // the preview waits like the real read does, so the loading state can be seen
  if (!isTauri()) await new Promise((r) => window.setTimeout(r, 900));
  return models;
}

// ---------- Claude / Codex connection ----------

/** Restart the Codex app-server / run the Claude probe now; no provider means both. */
export const reconnectQuota = (provider?: Provider): Promise<null> =>
  call("reconnect_quota", () => mockReconnect(provider), { provider });
/** Opens `claude auth login` / `codex login` in a console; the app reconnects when it closes. */
export const openLogin = (provider: Provider): Promise<null> =>
  call("open_login", () => mockReconnect(provider), { provider });
/** Claude Code and Codex on this machine: version, signed in, how "Cài" installs them. */
export const getAiClis = (): Promise<AiCli[]> => call("ai_clis", () => mockAiClis());
/** Runs the CLI's installer in a console (Codex needs npm; without it the install page opens). */
export const installCli = (provider: Provider): Promise<null> =>
  call("install_cli", () => null, { provider });
/** A link from a brief or a chat, in the default browser (https only). */
export const openLink = (url: string): Promise<null> =>
  call(
    "open_link",
    () => {
      window.open(url, "_blank", "noopener");
      return null;
    },
    { url },
  );

// ---------- start work ----------

/** Repos (workspace scan + remembered) and pre-filled branch for one task. */
export const getStartOptions = (wpId: number): Promise<StartOptions> =>
  call("start_options", () => mockStartOptions(wpId), { wpId });
/** Local + origin branches and uncommitted files of one repo. */
export const getRepoState = (repo: string, wpId?: number): Promise<RepoState> =>
  call("repo_state", () => mockRepoState(repo, wpId), { repo, wpId });
/** `git fetch --prune origin` (read-only on the remote), then the fresh state. */
export const fetchRepo = (repo: string, wpId?: number): Promise<RepoState> =>
  call("fetch_repo", () => mockFetchRepo(repo, wpId), { repo, wpId });
/** Deletes a branch the user confirmed: local (git branch -d/-D) or on origin (git push --delete). */
export const deleteBranch = (request: DeleteBranchRequest): Promise<RepoState> =>
  call("delete_branch", () => mockDeleteBranch(request), { request });
/** Creates (or switches to) the branch locally, writes brief.md, moves the task to "Đang làm". */
export const startTask = (request: StartRequest): Promise<StartResult> =>
  call("start_task", () => mockStartTask(request), { request });
/** Back to Inbox (branch, brief and workflow kept); refused while a step runs. */
export const pauseTask = (wpId: number): Promise<null> => call("pause_task", () => mockPauseTask(wpId), { wpId });
/** Reads the task's GitHub issues through the user's Chrome (Chrome asks Allow) into the brief. */
export const fetchIssues = (wpId: number): Promise<IssueFetch> =>
  call("fetch_issues", () => mockFetchIssues(wpId), { wpId });
/** Opens chrome://inspect/#remote-debugging in Chrome. */
export const openBrowserDebug = (): Promise<null> => call("open_browser_debug", () => null);
/** Puts the task folders (briefs) in `dir` (undefined = back to the data folder), moving the old ones. */
export const setTasksDir = (dir?: string): Promise<TasksDirChange> =>
  call("set_tasks_dir", () => mockSetTasksDir(dir), { dir });
/** Shows the task's brief.md in Explorer. */
export const openBrief = (wpId: number): Promise<null> => call("open_brief", () => null, { wpId });

// ---------- workflow (steps 2-8) ----------

/** Brief, plan, findings, push summary and log of one task in progress. */
export const getTaskDetail = (wpId: number): Promise<TaskDetail> =>
  call("task_detail", () => mockTaskDetail(wpId), { wpId });
/** Run / stop / retry a step, approve or reject the plan, finish the task, … */
export const flowAction = (wpId: number, action: FlowAction): Promise<TaskDetail> =>
  call("flow_action", () => mockFlowAction(wpId, action), { wpId, action });
/** Fix / accept / defer one review finding. */
export const decideFinding = (decision: FindingDecision): Promise<TaskDetail> =>
  call("decide_finding", () => mockDecideFinding(decision), { decision });
/** Save the brief the user edited before the AI reads it. */
export const saveBrief = (wpId: number, text: string): Promise<TaskDetail> =>
  call("save_brief", () => mockSaveBrief(wpId, text), { wpId, text });
/** Shows the task folder (brief, plan, push summary) in Explorer. */
export const openTaskFolder = (wpId: number): Promise<null> => call("open_task_folder", () => null, { wpId });
/** Opens a link of this task in the browser: the task on OpenProject (no `url`), a comment on
 *  it, or the GitHub issue. */
export const openTaskLink = (wpId: number, url?: string): Promise<null> =>
  call(
    "open_task_link",
    () => {
      window.open(url ?? `https://openproject.example.com/work_packages/${wpId}`, "_blank", "noopener");
      return null;
    },
    { wpId, url },
  );
/** Comment tab: OpenProject comments (read now) and the last issue read; `markSeen` clears "mới". */
export const getTaskComments = (wpId: number, markSeen: boolean): Promise<CommentsView> =>
  call("task_comments", () => mockTaskComments(wpId, markSeen), { wpId, markSeen });
/** What the task asks: its brief once started, OpenProject's description (`readNow`: asked now,
 *  else the last task list). Works for an Inbox task. */
export const getTaskText = (wpId: number, readNow: boolean): Promise<TaskText> =>
  call("task_text", () => mockTaskText(wpId, readNow), { wpId, readNow });
/** Rewrites the brief's OpenProject comment section with the comments there now. */
export const refreshBriefComments = (wpId: number): Promise<CommentsView> =>
  call("refresh_brief_comments", () => mockRefreshBriefComments(wpId), { wpId });
/** Diff tab: the task's change since its base (git, read-only); `markSeen` remembers it. */
export const getTaskDiff = (wpId: number, markSeen: boolean): Promise<DiffView> =>
  call("task_diff", () => mockTaskDiff(wpId, markSeen), { wpId, markSeen });
/** Chat tab: the transcript of one step run (`runId` undefined = the latest). */
export const getStepChat = (wpId: number, runId?: number): Promise<ChatView> =>
  call("step_chat", () => mockStepChat(wpId, runId), { wpId, runId });
/** "Cho phép lệnh này": adds `rule` to the repo's allowBash in config.json. */
export const allowBash = (wpId: number, rule: string): Promise<TaskDetail> =>
  call("allow_bash", () => mockAllowBash(wpId, rule), { wpId, rule });
/** "Đã push xong": `git fetch` the task branch and compare it with `origin/<branch>` (read only). */
export async function pushCheck(wpId: number): Promise<PushCheck> {
  const check = await call("push_check", () => mockPushCheck(wpId), { wpId });
  // the preview waits like a real fetch does, so the checking state can be seen
  if (!isTauri()) await new Promise((r) => window.setTimeout(r, 700));
  return check;
}
/** Opens the scratch folder, library folder or screenshot a Harness / Test trình duyệt result
 *  shows. */
export const openTaskPath = (wpId: number, path: string): Promise<null> =>
  call("open_task_path", () => null, { wpId, path });
/** Settings → Repo & nhánh: the harness library of one repo (`dir` absent = the default). */
export const setHarnessDir = (repo: string, dir?: string): Promise<SettingsView> =>
  call("set_harness_dir", () => mockSetHarnessDir(repo, dir), { repo, dir });
/** Settings → Repo → Lệnh kiểm tra of one repo (empty = guessed per package). */
export const setRepoChecks = (repo: string, commands: string[]): Promise<SettingsView> =>
  call("set_repo_checks", () => mockSetRepoChecks(repo, commands), { repo, commands });
/** Continues a finished run's AI session in a console window in the task's repo. */
export const openResume = (wpId: number, runId: number): Promise<null> =>
  call("open_resume", () => null, { wpId, runId });

// ---------- bell, notifications, tray ----------

export const getAlerts = (): Promise<AlertView[]> => call("alerts", mockAlerts);
/** Marks alerts read; no ids = all. */
export const markAlertsRead = (ids: number[] = []): Promise<null> =>
  call("mark_alerts_read", () => mockMarkAlertsRead(ids), { ids });

export const deleteAlerts = (ids: number[]): Promise<AlertView[]> =>
  call("delete_alerts", () => mockDeleteAlerts(ids), { ids });

export const restoreAlerts = (ids: number[]): Promise<AlertView[]> =>
  call("restore_alerts", () => mockRestoreAlerts(ids), { ids });
/** Turns one kind of Windows notification on or off. */
export const setNotify = (kind: NotifyKind, on: boolean): Promise<SettingsView> =>
  call("set_notify", () => ({ ...mockSettings(), notify: mockSetNotify(kind, on) }), { kind, on });

/** Subscribes to a Tauri event (nothing in the browser preview). Returns the unsubscribe function. */
function onEvent<T>(name: string, handler: (payload: T) => void): () => void {
  if (!isTauri()) return () => undefined;
  let stop: (() => void) | undefined;
  let done = false;
  void listen<T>(name, (e) => handler(e.payload)).then((unlisten) => {
    if (done) unlisten();
    else stop = unlisten;
  });
  return () => {
    done = true;
    stop?.();
  };
}

/** The window's X was pressed and `onClose` says "ask": show the close dialog. `running` = AI
 *  steps running now. Returns the unsubscribe function. */
export function onCloseAsked(handler: (running: number) => void): () => void {
  // browser preview: `?close=2` opens the dialog as if X was pressed with 2 steps running
  if (!isTauri() && typeof location !== "undefined") {
    const raw = new URLSearchParams(location.search).get("close");
    if (raw !== null) {
      const timer = window.setTimeout(() => handler(Number(raw) || 0), 300);
      return () => window.clearTimeout(timer);
    }
  }
  return onEvent<{ running: number }>("opw://close-asked", (p) => handler(p.running));
}
/** The close dialog's answer: tray (the AI keeps running) or quit (every AI step stops first).
 *  `remember` saves it as `onClose`. */
export const closeWindow = (choice: CloseChoice, remember: boolean): Promise<null> =>
  call("close_window", () => null, { choice, remember });
/** Settings → Thông báo → "Khi đóng cửa sổ". */
export const setOnClose = (choice: CloseChoice): Promise<SettingsView> =>
  call("set_on_close", () => mockSetOnClose(choice), { choice });
/** Settings → Phiên: hide (or show again) Claude sessions opened outside the app. */
export const setHideOutsideSessions = (hide: boolean): Promise<SettingsView> =>
  call("set_hide_outside_sessions", () => mockSetHideOutside(hide), { hide });

/** The tray icon asks to open the quota popover. Returns the unsubscribe function. */
export const onOpenQuota = (handler: () => void): (() => void) => onEvent("opw://open-quota", () => handler());

/** A Windows notification was clicked (the window is already in front): open what it is about.
 *  Returns the unsubscribe function. */
export const onAlertClicked = (handler: (alert: AlertView) => void): (() => void) =>
  onEvent<AlertView>("opw://open-alert", handler);

// ---------- Workspace sessions (PLAN.md §12 B) ----------

/** The tree: tasks in progress and every session with its state. */
export const getWorkspace = (): Promise<WorkspaceSnapshot> => call("workspace_snapshot", mockWorkspace);
/** Repos "+ Phiên mới" offers (scans the workspace folders). */
export const getWorkspaceRepos = (): Promise<string[]> => call("workspace_repos", mockWorkspaceRepos);
/** A task's new session, or one in a repo without a task. */
export const newSession = (request: NewSession): Promise<SessionView> =>
  call("session_new", () => mockSessionNew(request), { request });
/** Everything the chat pane shows. */
export const getSessionChat = (id: string): Promise<SessionChat> =>
  call("session_chat", () => mockSessionChat(id), { id });
/** Sends a message; the session starts or resumes first when nothing runs. `force` sends even
 *  though it looks open elsewhere (left behind by a crash). */
export const sendToSession = (id: string, text: string, force = false, images: ImageIn[] = []): Promise<SessionView> =>
  call("session_send", () => mockSessionSend(id, text), { id, text, force, images });
/** Answers a permission, a question, a plan or folder trust. */
export const answerSession = (id: string, askId: string, answer: AskAnswer): Promise<SessionView> =>
  call("session_answer", () => mockSessionAnswer(id, askId, answer), { id, askId, answer });
/** Stops the running turn (the session stays open). */
export const interruptSession = (id: string): Promise<null> =>
  call("session_interrupt", () => mockSessionInterrupt(id), { id });
/** Permission mode now (if it runs) and for every later start. */
export const setSessionMode = (id: string, mode: string): Promise<SessionView> =>
  call("session_set_mode", () => mockSessionSetMode(id, mode), { id, mode });
/** Ends the session's process here; the next message resumes it. */
export const stopSession = (id: string): Promise<SessionView> =>
  call("session_stop", () => mockSessionStop(id), { id });
/** Gives the session to VS Code: the app stops it, opens the repo's window, then the session. */
export const openSessionInVscode = (id: string): Promise<null> =>
  call("session_open_vscode", () => mockSessionOpenVscode(id), { id });
/** Takes the session off the Workspace list (its transcript stays). */
export const hideSession = (id: string): Promise<null> => call("session_hide", () => mockSessionHide(id), { id });
/** What the session changed (a task's change since its base; a free session's uncommitted one). */
export const getSessionDiff = (id: string): Promise<DiffView> =>
  call("session_diff", () => mockTaskDiff(0, false), { id });
/** Opens a file of the session's repo in VS Code, at `line`. */
export const openSessionFile = (id: string, path: string, line?: number): Promise<null> =>
  call("session_open_file", () => null, { id, path, line });
/** Opens a file of the task's repo in VS Code (task window → Diff). */
export const openTaskFile = (wpId: number, path: string, line?: number): Promise<null> =>
  call("task_open_file", () => null, { wpId, path, line });
/** Shows a changed file next to its version at `rev` (the diff's fork point) in VS Code's diff view. */
export const compareSessionFile = (id: string, rev: string, path: string, oldPath?: string): Promise<null> =>
  call("session_compare_file", () => null, { id, rev, path, oldPath });
/** The same for the task window's Diff tab. */
export const compareTaskFile = (wpId: number, rev: string, path: string, oldPath?: string): Promise<null> =>
  call("task_compare_file", () => null, { wpId, rev, path, oldPath });
/** Opens the session's repo folder in VS Code; the session stays in the app. */
export const openSessionRepo = (id: string): Promise<null> => call("session_open_repo", () => null, { id });
/** Repo files for `@` in the chat. */
export const findSessionFiles = (id: string, query: string): Promise<string[]> =>
  call("session_files", () => mockFiles(query), { id, query });
/** Commands for `/` in the chat. */
export const getSessionCommands = (id: string): Promise<SlashCommand[]> =>
  call("session_commands", () => MOCK_COMMANDS, { id });
/** The repo's package.json scripts. */
export const getSessionScripts = (id: string): Promise<RepoScript[]> =>
  call("session_scripts", () => MOCK_SCRIPTS, { id });
/** A Windows notification for a session that started waiting (when those are on). */
export const notifySessionWait = (id: string): Promise<null> => call("session_notify_wait", () => null, { id });
/** The conversation of a Claude session opened outside the app (read-only). */
export const getOutsideChat = (id: string): Promise<OutsideChat> =>
  call("outside_chat", () => mockOutsideChat(id), { id });
/** Takes a session opened outside the app off the list (it stays open where it is). */
export const archiveOutside = (id: string): Promise<null> =>
  call("outside_archive", () => mockArchiveOutside(id), { id });
/** Puts an archived session back on the list. */
export const restoreSession = (id: string): Promise<null> =>
  call("session_restore", () => mockRestoreSession(id), { id });
/** "Tiếp tục trong app": the session joins the app's list (typed into here once closed outside). */
export const adoptOutside = (id: string): Promise<SessionView> =>
  call("outside_adopt", () => mockAdoptOutside(id), { id });
/** "Tách bản sao": a copy of the conversation, typed into here at once. */
export const forkOutside = (id: string): Promise<SessionView> =>
  call("outside_fork", () => mockForkOutside(id), { id });
/** A session's notification was clicked: open that session. */
export const onSessionClicked = (handler: (id: string) => void): (() => void) =>
  onEvent<string>("opw://open-session", handler);

// ---------- preview data for the Workspace tools ----------

const MOCK_FILES = [
  "src/orders/order.repository.ts", "src/orders/order.service.ts", "src/orders/order.controller.ts",
  "src/users/user.entity.ts", "src/app.module.ts", "test/orders.e2e-spec.ts", "package.json", "README.md",
];
function mockFiles(query: string): string[] {
  const q = query.toLowerCase();
  return MOCK_FILES.filter((f) => f.toLowerCase().includes(q)).slice(0, 30);
}
const MOCK_COMMANDS: SlashCommand[] = [
  { name: "compact", description: "Tóm tắt hội thoại để có lại chỗ trong context", source: "claude" },
  { name: "context", description: "Xem context của phiên đang dùng bao nhiêu", source: "claude" },
  { name: "review-local", description: "Review the local change against the task", source: "repo" },
  { name: "init", description: "Viết CLAUDE.md cho repo", source: "claude" },
];
const MOCK_SCRIPTS: RepoScript[] = [
  { name: "dev", command: "nest start --watch", run: "pnpm run dev" },
  { name: "test", command: "jest", run: "pnpm run test" },
  { name: "lint", command: "eslint .", run: "pnpm run lint" },
];
/** Changes of sessions the app runs, as they happen. Returns the unsubscribe function. */
export function onSessionPatch(handler: (patch: SessionPatch) => void): () => void {
  if (!isTauri()) return onMockSessionPatch(handler);
  return onEvent<SessionPatch>("opw://session", handler);
}

// ---------- update check ----------

export const getUpdateStatus = (): Promise<UpdateView> => call("update_status", mockUpdate);
/** Asks GitHub Releases now. */
export const checkUpdate = (): Promise<UpdateView> => call("check_update", mockUpdate);
export const openReleasePage = (): Promise<null> =>
  call("open_release_page", () => {
    window.open("https://github.com/wangnguen/op-workflow/releases", "_blank", "noopener");
    return null;
  });

/** Message of a failed command (Tauri rejects with the `{code, message}` object). */
export function errorMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) return String((e as ApiError).message);
  return String(e);
}

/** Code of a failed command, e.g. "repo.dirty". */
export function errorCode(e: unknown): string | undefined {
  if (typeof e === "object" && e !== null && "code" in e) return String((e as ApiError).code);
  return undefined;
}

/** True when running in a browser with mock data instead of the Tauri shell. */
export const isMockMode = (): boolean => !isTauri();

export const minimizeWindow = (): Promise<void> => getCurrentWindow().minimize();
export const toggleMaximizeWindow = (): Promise<void> => getCurrentWindow().toggleMaximize();
// Use the real close request so the Rust window event handles the saved close preference.
export const requestWindowClose = (): Promise<void> => getCurrentWindow().close();

export function onWindowMaximized(onChange: (maximized: boolean) => void): () => void {
  const appWindow = getCurrentWindow();
  let alive = true;
  let request = 0;
  const refresh = () => {
    if (!alive) return;
    const current = ++request;
    void appWindow.isMaximized().then((maximized) => {
      if (alive && current === request) onChange(maximized);
    }).catch(() => undefined);
  };
  const listening = appWindow.onResized(refresh);
  void listening.then(refresh, refresh);
  return () => {
    alive = false;
    void listening.then((unlisten) => unlisten()).catch(() => undefined);
  };
}

/** UI refresh period for snapshots. */
export const POLL_MS = 1500;
