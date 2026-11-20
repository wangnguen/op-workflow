import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage, getSessionChat, onSessionPatch } from "../../ipc/client";
import type { SessionChat, SessionPatch, SessionView } from "../../ipc/types";
import { applyPatch } from "./chatState";

/** A session the app does not run (idle here, or open in VS Code) is read again this often:
 *  only a transcript that changed is parsed again. */
const QUIET_POLL_MS = 2000;

export interface LiveChat {
  chat: SessionChat | undefined;
  error: string | undefined;
  /** read the whole chat again */
  reload: () => void;
  /** a command answered with the session's new state before its patch arrived */
  setSession: (s: SessionView) => void;
}

/** The chat of session `id`: read once, then kept up to date by the pushed patches while the app
 *  runs it, and read again now and then while it does not. */
export function useSessionChat(id: string | undefined): LiveChat {
  const [chat, setChat] = useState<SessionChat>();
  const [error, setError] = useState<string>();
  const current = useRef<SessionChat | undefined>(undefined);
  const reloadRef = useRef<() => void>(() => undefined);

  useEffect(() => {
    current.current = undefined;
    setChat(undefined);
    setError(undefined);
    if (!id) return;
    let alive = true;
    let loading = false;
    const early: SessionPatch[] = [];
    const show = (c: SessionChat) => {
      current.current = c;
      setChat(c);
    };
    const apply = (p: SessionPatch) => {
      const next = applyPatch(current.current, p);
      if (next === "refetch") void read();
      else if (next !== current.current) show(next);
    };
    const read = async () => {
      if (loading) return;
      loading = true;
      try {
        const c = await getSessionChat(id);
        if (!alive) return;
        // a newer state already arrived by patch: keep it
        if (!current.current || c.seq >= current.current.seq) show(c);
        setError(undefined);
        for (const p of early.splice(0)) apply(p);
      } catch (e) {
        if (alive) setError(errorMessage(e));
      } finally {
        loading = false;
      }
    };
    reloadRef.current = () => void read();
    // listen first, so nothing pushed while the chat is read is lost
    const stop = onSessionPatch((p) => {
      if (p.id !== id || !alive) return;
      if (!current.current) early.push(p);
      else apply(p);
    });
    void read();
    const timer = window.setInterval(() => {
      if (current.current && !current.current.session.live) void read();
    }, QUIET_POLL_MS);
    return () => {
      alive = false;
      stop();
      window.clearInterval(timer);
    };
  }, [id]);

  const reload = useCallback(() => reloadRef.current(), []);
  const setSession = useCallback((s: SessionView) => {
    const c = current.current;
    if (!c || c.session.id !== s.id) return;
    const next = { ...c, session: s };
    current.current = next;
    setChat(next);
  }, []);
  return { chat, error, reload, setSession };
}
