// The only module that talks to Tauri. Outside Tauri (plain browser) it serves mock data.
import { invoke, isTauri } from "@tauri-apps/api/core";
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
  mockSettings,
  mockStartOptions,
  mockStartTask,
  mockUpdate,
} from "./mock";
import type {
  ApiError,
  AppInfo,
  BoardSnapshot,
  CommandArgs,
  Commands,
  DeleteBranchRequest,
  IssueFetch,
  OrbitSnapshot,
  Provider,
  RepoState,
  SettingsView,
  StartOptions,
  StartRequest,
  StartResult,
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
export const pauseTask = (wpId: number): Promise<null> => call("pause_task", () => null, { wpId });
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
