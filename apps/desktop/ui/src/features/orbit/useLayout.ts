import { useCallback, useState } from "react";
import type { Point } from "./geometry";

/** Positions the user dragged on the map, by item key (`nebula:<taskId>`, `distant:<id>`). */
export type LayoutOverrides = Record<string, Point>;

const STORAGE_KEY = "opw.orbit.layout.v1";

function load(): LayoutOverrides {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    if (typeof parsed !== "object" || parsed === null) return {};
    const out: LayoutOverrides = {};
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof v === "object" && v !== null) {
        const { x, y } = v as { x?: unknown; y?: unknown };
        if (typeof x === "number" && typeof y === "number" && Number.isFinite(x) && Number.isFinite(y)) {
          out[k] = { x, y };
        }
      }
    }
    return out;
  } catch {
    return {};
  }
}

function save(layout: LayoutOverrides): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(layout));
  } catch {
    // private mode / storage blocked: positions just do not survive a restart
  }
}

/** Dragged positions, kept in this browser profile only (a per-viewer convenience). */
export function useLayout() {
  const [overrides, setOverrides] = useState<LayoutOverrides>(load);

  const move = useCallback((key: string, p: Point, persist: boolean) => {
    setOverrides((prev) => {
      const next = { ...prev, [key]: { x: Math.round(p.x), y: Math.round(p.y) } };
      if (persist) save(next);
      return next;
    });
  }, []);

  const reset = useCallback(() => {
    setOverrides({});
    save({});
  }, []);

  return { overrides, move, reset, customized: Object.keys(overrides).length > 0 };
}
