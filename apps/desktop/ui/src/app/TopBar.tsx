import { vi } from "../i18n/vi";
import type { ProviderLink, QuotaSnapshot, UpdateView } from "../ipc/types";
import { QuotaGauges } from "../features/quota/QuotaGauges";
import { OPEN_QUOTA_EVENT } from "../features/quota/quota";
import { AlertBell } from "./AlertBell";
import { WindowControls } from "./WindowControls";

export type Tab = "task" | "workspace" | "sessions";

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  quota: QuotaSnapshot[];
  links: ProviderLink[];
  now: number;
  demo: boolean;
  needsYou: boolean;
  /** a Workspace session waits for an answer */
  workspaceNeedsYou: boolean;
  /** unread alerts in the bell */
  unreadAlerts: number;
  onOpenTask: (wpId: number, tab: string | undefined) => void;
  update: UpdateView | undefined;
  onRefresh: () => void;
  onSettings: () => void;
  onOpenRelease: () => void;
  /** the window has no frame: the bar carries its buttons and drags it */
  frameless: boolean;
}

const TABS: { key: Tab; label: string }[] = [
  { key: "task", label: vi.tabs.task },
  { key: "workspace", label: vi.tabs.workspace },
  { key: "sessions", label: vi.tabs.sessions },
];

/** Outline of an 8-tooth cog in a 24×24 box (drawn here, no icon font). */
const GEAR_PATH = (() => {
  const teeth = 8;
  const step = (2 * Math.PI) / teeth;
  const at = (angle: number, r: number) =>
    `${(12 + r * Math.cos(angle)).toFixed(2)} ${(12 + r * Math.sin(angle)).toFixed(2)}`;
  const points: string[] = [];
  for (let i = 0; i < teeth; i++) {
    const a = i * step;
    points.push(at(a - step * 0.3, 7.2), at(a - step * 0.16, 9.8), at(a + step * 0.16, 9.8), at(a + step * 0.3, 7.2));
  }
  return `M${points.join(" L")} Z`;
})();

export function TopBar({
  tab,
  onTab,
  quota,
  links,
  now,
  demo,
  needsYou,
  workspaceNeedsYou,
  unreadAlerts,
  onOpenTask,
  update,
  onRefresh,
  onSettings,
  onOpenRelease,
  frameless,
}: Props) {
  return (
    // "deep": a press anywhere in the bar that is not a button, tab or field drags the window
    <header className={`topbar${frameless ? " topbar--frameless" : ""}`} data-tauri-drag-region={frameless ? "deep" : undefined}>
      <div className="brand">
        {/* the app icon in small: a task flows from a done step through the current one to done */}
        <svg viewBox="0 0 24 24" width={20} height={20} aria-hidden="true" className="brand-mark">
          <path d="M6.5 17.5C6.5 14.6 9.5 13.2 12 12s5.5-2.6 5.5-5.5" className="brand-flow" />
          <circle cx={6.5} cy={17.5} r={2.4} className="brand-done" />
          <circle cx={12} cy={12} r={1.9} className="brand-live" />
          <circle cx={17.5} cy={6.5} r={4.2} className="brand-goal" />
          <path d="M15.6 6.6l1.3 1.3 2.4-2.6" className="brand-check" />
        </svg>
        <span>{vi.app.name}</span>
      </div>

      <nav className="tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.key}
            role="tab"
            aria-selected={tab === t.key}
            className={`tab${tab === t.key ? " is-on" : ""}`}
            onClick={() => onTab(t.key)}
          >
            {t.label}
            {t.key === "sessions" && needsYou && <span className="tab-dot" aria-label={vi.sessions.needsYou} />}
            {t.key === "workspace" && workspaceNeedsYou && (
              <span className="tab-dot" aria-label={vi.sessions.needsYou} />
            )}
          </button>
        ))}
      </nav>

      <div className="topbar-spacer" />

      {update?.available && update.latest && (
        <button className="chip chip--update" title={vi.topbar.updateHint} onClick={onOpenRelease}>
          <svg viewBox="0 0 24 24" width={12} height={12} aria-hidden="true">
            <path d="M12 4v11M7 10l5 5 5-5M5 20h14" className="icon-stroke" />
          </svg>
          {vi.topbar.update(update.latest)}
        </button>
      )}
      {demo && (
        <span className="chip chip--demo" title={vi.app.demoHint}>
          {vi.app.demo}
        </span>
      )}
      <QuotaGauges quota={quota} links={links} now={now} onChanged={onRefresh} />
      <span className="kbd-hint" title={vi.topbar.searchHint}>
        <kbd>{vi.topbar.search}</kbd>
      </span>
      <AlertBell
        unread={unreadAlerts}
        needsYou={needsYou}
        now={now}
        onOpenTask={onOpenTask}
        onOpenQuota={() => window.dispatchEvent(new Event(OPEN_QUOTA_EVENT))}
        onChanged={onRefresh}
      />
      <button
        className="btn btn--icon btn--ghost"
        aria-label={vi.topbar.settings}
        title={vi.topbar.settings}
        onClick={onSettings}
      >
        <svg viewBox="0 0 24 24" width={16} height={16} aria-hidden="true">
          <path d={GEAR_PATH} className="icon-stroke" />
          <circle cx={12} cy={12} r={3} className="icon-stroke" />
        </svg>
        {update?.available && <span className="bell-dot bell-dot--update" />}
      </button>
      {frameless && <WindowControls />}
    </header>
  );
}
