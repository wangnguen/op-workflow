import { useEffect, useRef, useState } from "react";
import { vi } from "../i18n/vi";
import { deleteAlerts, getAlerts, markAlertsRead, restoreAlerts } from "../ipc/client";
import type { AlertView } from "../ipc/types";
import { formatRelative } from "../shared/format";
import { errorMessage, useDismiss } from "../shared/hooks";
import { Icon } from "../shared/ui/Icon";
import { SegmentedControl } from "../shared/ui/SegmentedControl";
import { visibleAlerts, type AlertFilter } from "./alerts";

const t = vi.alerts;

interface Props {
  /** unread count from the board snapshot */
  unread: number;
  /** a task waits for the user (gate): dot even without unread alerts */
  needsYou: boolean;
  now: number;
  /** open the task window of an alert, at its tab */
  onOpenTask: (wpId: number, tab: string | undefined) => void;
  /** a quota alert: open the quota popover */
  onOpenQuota: () => void;
  /** the unread count changed */
  onChanged: () => void;
}

/** The persistent bell: active alerts and a recoverable Deleted list. */
export function AlertBell({ unread, needsYou, now, onOpenTask, onOpenQuota, onChanged }: Props) {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<AlertView[]>([]);
  const [filter, setFilter] = useState<AlertFilter>("unread");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const busyRef = useRef(false);
  const requestId = useRef(0);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));

  useEffect(() => {
    if (!open) return;
    let alive = true;
    const refresh = () => {
      if (busyRef.current) return;
      const id = ++requestId.current;
      void getAlerts().then((list) => {
        if (alive && requestId.current === id) setItems(list);
      }).catch((e: unknown) => {
        if (alive && requestId.current === id) setError(errorMessage(e));
      });
    };
    refresh();
    const timer = window.setInterval(refresh, 3000);
    return () => { alive = false; ++requestId.current; window.clearInterval(timer); };
  }, [open]);

  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    // open on what needs reading; with nothing unread, show the whole session
    setFilter(unread > 0 ? "unread" : "all");
    setError(undefined);
    setOpen(true);
  };

  const pick = (a: AlertView) => {
    void markAlertsRead([a.id]).then(onChanged).catch((e: unknown) => setError(errorMessage(e)));
    setOpen(false);
    if (a.kind === "quota") onOpenQuota();
    else if (a.wpId !== undefined) onOpenTask(a.wpId, a.tab);
  };

  const mutate = async (operation: () => Promise<AlertView[]>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    ++requestId.current; // an older poll cannot put a deleted alert back in the list
    setBusy(true);
    setError(undefined);
    try {
      setItems(await operation());
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const activeItems = visibleAlerts(items, "all").length;
  const unreadItems = visibleAlerts(items, "unread").length;
  const deletedItems = visibleAlerts(items, "deleted").length;
  const shown = visibleAlerts(items, filter);
  const label = unread > 0 ? vi.topbar.notificationsUnread(unread) : vi.topbar.notifications;
  return (
    <div className="bell" ref={ref}>
      <button className="btn btn--icon btn--ghost" aria-label={label} title={label} aria-expanded={open} onClick={toggle}>
        <svg viewBox="0 0 24 24" width={16} height={16} aria-hidden="true">
          <path d="M6 16V11a6 6 0 1 1 12 0v5l1.5 2h-15z M10 20a2 2 0 0 0 4 0" className="icon-stroke" />
        </svg>
        {unread > 0 ? <span className="bell-count">{unread > 9 ? "9+" : unread}</span> : needsYou && <span className="bell-dot" />}
      </button>
      {open && (
        <div className="bell-pop" role="dialog" aria-label={t.title}>
          <header>
            <h3>{t.title}</h3>
            <button className="btn btn--icon btn--ghost" aria-label={t.close} onClick={() => setOpen(false)}>
              <Icon name="close" />
            </button>
          </header>
          <SegmentedControl
            options={[
              { value: "unread", label: t.unread, count: unreadItems > 0 ? unreadItems : undefined },
              { value: "all", label: t.all, count: activeItems > 0 ? activeItems : undefined },
              { value: "deleted", label: t.deleted, count: deletedItems > 0 ? deletedItems : undefined },
            ]}
            value={filter}
            onChange={setFilter}
            label={t.filterLabel}
            fill
          />
          {/* the same two buttons in every list, so switching lists moves nothing */}
          <div className="bell-toolbar">
            <button className="btn btn--ghost btn--sm" disabled={busy || filter === "deleted" || unreadItems === 0}
              onClick={() => void mutate(() => markAlertsRead([]).then(getAlerts))}>{t.markAll}</button>
            <button className="btn btn--ghost btn--sm bell-bulk" disabled={busy || shown.length === 0}
              title={filter === "deleted" ? t.restoreAllHint : t.deleteAllHint}
              onClick={() => void mutate(() => filter === "deleted"
                ? restoreAlerts(shown.map((a) => a.id))
                : deleteAlerts(shown.map((a) => a.id)))}>
              <Icon name={filter === "deleted" ? "refresh" : "trash"} size={12} />
              {filter === "deleted" ? t.restoreAll : t.deleteAll}
            </button>
          </div>
          {error && <p className="bell-error" role="alert">{error}</p>}
          {shown.length === 0 ? (
            <p className="dim bell-empty">{filter === "deleted" ? t.emptyDeleted : filter === "unread" && activeItems > 0 ? t.emptyUnread : t.empty}</p>
          ) : (
            <ul className="bell-list">
              {shown.map((a) => (
                <li key={a.id} className="bell-row">
                  <button className={`bell-item bell-item--${a.kind}${a.read || a.deletedAt ? "" : " is-unread"}`} disabled={busy} onClick={() => pick(a)}>
                    <span className="bell-item-top">
                      <span className={`bell-kind bell-kind--${a.kind}`}>{t.kind[a.kind]}</span>
                      <span className="faint">{formatRelative(a.at, now)}</span>
                    </span>
                    <strong>{a.title}</strong>
                    {a.body && <span className="dim bell-body">{a.body}</span>}
                    {a.deletedAt && <span className="faint bell-deleted-at">{t.deletedWhen(formatRelative(a.deletedAt, now))}</span>}
                  </button>
                  <button className="btn btn--icon btn--ghost bell-row-action" disabled={busy}
                    aria-label={`${a.deletedAt ? t.restore : t.delete}: ${a.title}`} title={a.deletedAt ? t.restore : t.delete}
                    onClick={() => void mutate(() => a.deletedAt ? restoreAlerts([a.id]) : deleteAlerts([a.id]))}>
                    <Icon name={a.deletedAt ? "refresh" : "trash"} size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <p className="faint bell-note">{filter === "deleted" ? t.deletedNote : t.note}</p>
        </div>
      )}
    </div>
  );
}
