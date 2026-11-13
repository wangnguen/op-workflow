import { useCallback, useEffect, useRef, useState } from "react";

/** JSON of the last value, to skip re-rendering when a poll returns the same data. */
function sameJson(a: unknown, b: unknown): boolean {
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

export interface Polled<T> {
  data: T | undefined;
  error: string | undefined;
  refresh: () => void;
}

/** Calls `fetcher` now and then every `intervalMs`. `fetcher` must be stable. */
export function usePolling<T>(fetcher: () => Promise<T>, intervalMs: number): Polled<T> {
  const [data, setData] = useState<T>();
  const [error, setError] = useState<string>();
  const alive = useRef(true);
  const last = useRef<T | undefined>(undefined);

  const run = useCallback(async () => {
    try {
      const value = await fetcher();
      if (!alive.current) return;
      // unchanged data keeps the same object, so nothing below re-renders
      if (!sameJson(last.current, value)) {
        last.current = value;
        setData(value);
      }
      setError(undefined);
    } catch (e) {
      if (alive.current) setError(errorMessage(e));
    }
  }, [fetcher]);

  useEffect(() => {
    alive.current = true;
    void run();
    const id = window.setInterval(() => void run(), intervalMs);
    return () => {
      alive.current = false;
      window.clearInterval(id);
    };
  }, [run, intervalMs]);

  return { data, error, refresh: () => void run() };
}

/** Current time, re-rendered every `intervalMs`. */
export function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}

/** Calls `onOutside` on pointer down outside `ref` or on Escape. */
export function useDismiss(
  ref: React.RefObject<HTMLElement | null>,
  open: boolean,
  onOutside: () => void,
): void {
  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target)) onOutside();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOutside();
    };
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [ref, open, onOutside]);
}

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (typeof e === "object" && e !== null && "message" in e) {
    const m = (e as { message: unknown }).message;
    if (typeof m === "string") return m;
  }
  return String(e);
}
