import { useRef, useState } from "react";
import { vi } from "../i18n/vi";
import { getAlerts, markAlertsRead } from "../ipc/client";
import type { AlertView } from "../ipc/types";
import { formatRelative } from "../shared/format";
import { useDismiss } from "../shared/hooks";
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

/** The bell: what the app announced this session (also shown as Windows notifications). */
export function AlertBell({ unread, needsYou, now, onOpenTask, onOpenQuota, onChanged }: Props) {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<AlertView[]>([]);
  const [filter, setFilter] = useState<AlertFilter>("unread");
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));

  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    // open on what needs reading; with nothing unread, show the whole session
    setFilter(unread > 0 ? "unread" : "all");
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

  const unreadItems = items.filter((a) => !a.read).length;
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
            <button className="btn btn--ghost btn--sm" disabled={unreadItems === 0} onClick={readAll}>
              {t.markAll}
            </button>
          </header>
          <SegmentedControl
            options={[
              { value: "unread", label: t.unread, count: unreadItems > 0 ? unreadItems : undefined },
              { value: "all", label: t.all, count: items.length > 0 ? items.length : undefined },
            ]}
            value={filter}
            onChange={setFilter}
            label={t.filterLabel}
            fill
          />
          {shown.length === 0 ? (
            <p className="dim bell-empty">{filter === "unread" && items.length > 0 ? t.emptyUnread : t.empty}</p>
          ) : (
            <ul className="bell-list">
              {shown.map((a) => (
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
