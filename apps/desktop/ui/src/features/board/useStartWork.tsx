// "Bắt đầu làm việc" from anywhere (Task board, Phiên): the dialog, the result banner, and
// reading the task's GitHub issues right after it starts.
import { useEffect, useState, type ReactNode } from "react";
import { vi } from "../../i18n/vi";
import { errorCode, errorMessage, fetchIssues, isMockMode, openBrowserDebug } from "../../ipc/client";
import type { StartResult, TaskCard } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { StartWorkDialog } from "./StartWorkDialog";

export interface Flash {
  /** "busy" stays until replaced (Chrome may wait for the user's Allow) */
  kind: "ok" | "warn" | "busy";
  text: string;
  action?: { label: string; run: () => void };
}

const FLASH_MS = 8000;
const b = vi.board;

export const joined = (...parts: (string | undefined)[]) => parts.filter(Boolean).join(". ");

/** The board-style banner for a flash message. */
export function FlashBanner({ flash, onClose }: { flash: Flash; onClose: () => void }) {
  return (
    <div className={`board-banner board-banner--${flash.kind}`} role="status">
      <span className="board-banner-text">
        {flash.kind === "busy" && <Icon name="refresh" size={13} className="is-spinning" />}
        {flash.text}
      </span>
      <span className="board-banner-actions">
        {flash.action && (
          <button className="btn btn--sm" onClick={flash.action.run}>
            {flash.action.label}
          </button>
        )}
        <button className="btn btn--sm btn--ghost btn--icon" onClick={onClose} aria-label={vi.settings.close}>
          <Icon name="close" size={13} />
        </button>
      </span>
    </div>
  );
}

interface Options {
  /** every task on the board; the dialog warns about other tasks in "Đang làm" */
  tasks: TaskCard[];
  /** reload board + orbit */
  onChanged: () => void;
  /** after a task started (e.g. Phiên centres it on the map) */
  onStarted?: (taskId: string) => void;
}

export interface StartWork {
  open: (card: TaskCard) => void;
  readIssues: (card: Pick<TaskCard, "wpId" | "id">) => void;
  reading: ReadonlySet<number>;
  setFlash: (flash: Flash | null) => void;
  /** banner to render above the content (null when there is nothing to say) */
  banner: ReactNode;
  /** the dialog while a task is being started */
  dialog: ReactNode;
}

export function useStartWork({ tasks, onChanged, onStarted }: Options): StartWork {
  const [starting, setStarting] = useState<TaskCard | null>(null);
  const [flash, setFlash] = useState<Flash | null>(null);
  const [reading, setReading] = useState<ReadonlySet<number>>(new Set());

  useEffect(() => {
    if (!flash || flash.kind === "busy") return;
    const id = window.setTimeout(() => setFlash(null), FLASH_MS);
    return () => window.clearTimeout(id);
  }, [flash]);

  const fail = (e: unknown) => setFlash({ kind: "warn", text: errorMessage(e) });

  /** Read the task's GitHub issues through the user's Chrome into its brief. */
  const readIssues = ({ wpId, id }: Pick<TaskCard, "wpId" | "id">, before?: string) => {
    setReading((r) => new Set(r).add(wpId));
    setFlash({ kind: "busy", text: joined(before, b.issuesReading(id)) });
    fetchIssues(wpId)
      .then((r) => {
        const done = b.issuesDone(id, r.read);
        setFlash(
          r.warnings.length > 0
            ? { kind: "warn", text: joined(before, done, r.warnings.join(" ")) }
            : { kind: "ok", text: joined(before, done) },
        );
      })
      .catch((e: unknown) => {
        const action =
          errorCode(e) === "browser.notEnabled"
            ? { label: b.issuesOpenSettings, run: () => void openBrowserDebug().catch(fail) }
            : undefined;
        setFlash({ kind: "warn", text: joined(before, `${id}: ${errorMessage(e)}`), action });
      })
      .finally(() =>
        setReading((r) => {
          const next = new Set(r);
          next.delete(wpId);
          return next;
        }),
      );
  };

  const started = (r: StartResult) => {
    const card = starting;
    setStarting(null);
    const done = joined(vi.start.done(r.taskId, r.branch, r.created), ...r.warnings);
    if (card && (r.issueLinks ?? 0) > 0 && !isMockMode()) {
      readIssues(card, done);
    } else {
      setFlash({ kind: r.warnings.length > 0 ? "warn" : "ok", text: done });
    }
    onChanged();
    onStarted?.(r.taskId);
  };

  return {
    open: setStarting,
    readIssues: (card) => readIssues(card),
    reading,
    setFlash,
    banner: flash ? <FlashBanner flash={flash} onClose={() => setFlash(null)} /> : null,
    dialog: starting ? (
      <StartWorkDialog
        task={starting}
        inProgress={tasks.filter((t) => t.column === "active" && t.wpId !== starting.wpId)}
        onClose={() => setStarting(null)}
        onStarted={started}
      />
    ) : null,
  };
}
