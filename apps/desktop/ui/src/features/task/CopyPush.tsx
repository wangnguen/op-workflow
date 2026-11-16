import { useEffect, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage } from "../../ipc/client";
import type { PushView } from "../../ipc/types";
import { copyText } from "../../shared/clipboard";
import { Icon } from "../../shared/ui/Icon";
import { pushParts, type PushParts } from "./push";

const t = vi.flow;
const COPIED_MS = 1800;

interface Props {
  /** the push summary when the caller has it (task window) */
  push?: PushView;
  /** otherwise read it on click (the card has no push text) */
  load?: () => Promise<PushView | undefined>;
  onError?: (message: string) => void;
}

/** "Copy commit" and "Copy PR": put the text on the clipboard and say so on the button. */
export function CopyPush({ push, load, onError }: Props) {
  const [copied, setCopied] = useState<keyof PushParts>();
  const timer = useRef<number>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const parts = push ? pushParts(push) : undefined;

  const copy = async (which: keyof PushParts) => {
    try {
      const text = (parts ?? pushParts(await load?.()))[which];
      if (!text) throw new Error(t.copyMissing);
      await copyText(text);
      setCopied(which);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(undefined), COPIED_MS);
    } catch (e: unknown) {
      onError?.(e instanceof Error ? (e.message === "clipboard" ? t.copyFailed : e.message) : errorMessage(e));
    }
  };

  const button = (which: keyof PushParts, label: string, hint: string) => {
    const done = copied === which;
    return (
      <button
        className={`btn btn--sm${done ? " is-copied" : ""}`}
        title={hint}
        disabled={parts !== undefined && !parts[which]}
        onClick={() => void copy(which)}
      >
        <Icon name={done ? "check" : "file"} size={12} />
        {/* both labels sit in the same cell, so the button keeps its width */}
        <span className="copy-label">
          <span className={done ? "is-off" : undefined}>{label}</span>
          <span className={done ? undefined : "is-off"} aria-hidden={!done}>
            {t.copied}
          </span>
        </span>
      </button>
    );
  };

  return (
    <>
      {button("commit", t.copyCommit, t.copyCommitHint)}
      {button("pr", t.copyPr, t.copyPrHint)}
    </>
  );
}
