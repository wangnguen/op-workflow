// The only module that talks to Tauri. Outside Tauri (plain browser) it serves mock data.
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
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
  mockSetOnClose,
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
import type {
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

/** The tray icon asks to open the quota popover. Returns the unsubscribe function. */
export const onOpenQuota = (handler: () => void): (() => void) => onEvent("opw://open-quota", () => handler());

/** A Windows notification was clicked (the window is already in front): open what it is about.
 *  Returns the unsubscribe function. */
export const onAlertClicked = (handler: (alert: AlertView) => void): (() => void) =>
  onEvent<AlertView>("opw://open-alert", handler);

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

/** UI refresh period for snapshots. */
export const POLL_MS = 1500;
