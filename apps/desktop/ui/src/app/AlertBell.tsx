import { useRef, useState } from "react";
import { vi } from "../i18n/vi";
import { getAlerts, markAlertsRead } from "../ipc/client";
import type { AlertView } from "../ipc/types";
import { formatRelative } from "../shared/format";
import { useDismiss } from "../shared/hooks";

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

/** The bell: what the app announced this session (also shown as Windows notifications). */
export function AlertBell({ unread, needsYou, now, onOpenTask, onOpenQuota, onChanged }: Props) {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<AlertView[]>([]);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));

  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    void getAlerts().then(setItems);
  };

  const pick = (a: AlertView) => {
    void markAlertsRead([a.id]).then(onChanged);
    setOpen(false);
    if (a.kind === "quota") onOpenQuota();
    else if (a.wpId !== undefined) onOpenTask(a.wpId, a.tab);
  };

  const readAll = () => {
    void markAlertsRead([])
      .then(getAlerts)
      .then((list) => {
        setItems(list);
        onChanged();
      });
  };

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
            <button className="btn btn--ghost btn--sm" disabled={!items.some((a) => !a.read)} onClick={readAll}>
              {t.markAll}
            </button>
          </header>
          {items.length === 0 ? (
            <p className="dim bell-empty">{t.empty}</p>
          ) : (
            <ul className="bell-list">
              {items.map((a) => (
                <li key={a.id}>
                  <button className={`bell-item bell-item--${a.kind}${a.read ? "" : " is-unread"}`} onClick={() => pick(a)}>
                    <span className="bell-item-top">
                      <span className={`bell-kind bell-kind--${a.kind}`}>{t.kind[a.kind]}</span>
                      <span className="faint">{formatRelative(a.at, now)}</span>
                    </span>
                    <strong>{a.title}</strong>
                    {a.body && <span className="dim bell-body">{a.body}</span>}
                  </button>
                </li>
              ))}
            </ul>
          )}
          <p className="faint bell-note">{t.note}</p>
        </div>
      )}
    </div>
  );
}
