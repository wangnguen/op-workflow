// The only module that talks to Tauri. Outside Tauri (plain browser) it serves mock data.
import { invoke, isTauri } from "@tauri-apps/api/core";
import { mockAppInfo, mockBoard, mockClearToken, mockOrbit, mockSaveToken, mockSettings } from "./mock";
import type { ApiError, AppInfo, BoardSnapshot, CommandArgs, Commands, OrbitSnapshot, SettingsView } from "./types";

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

/** Message of a failed command (Tauri rejects with the `{code, message}` object). */
export function errorMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) return String((e as ApiError).message);
  return String(e);
}

/** True when running in a browser with mock data instead of the Tauri shell. */
export const isMockMode = (): boolean => !isTauri();

/** UI refresh period for snapshots. */
export const POLL_MS = 1500;
