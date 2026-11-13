import { vi } from "../i18n/vi";
import type { QuotaSnapshot } from "../ipc/types";
import { QuotaGauges } from "../features/quota/QuotaGauges";

export type Tab = "task" | "workspace" | "sessions";

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  quota: QuotaSnapshot[];
  now: number;
  demo: boolean;
  needsYou: boolean;
  onRefresh: () => void;
  onSettings: () => void;
}

const TABS: { key: Tab; label: string }[] = [
  { key: "task", label: vi.tabs.task },
  { key: "workspace", label: vi.tabs.workspace },
  { key: "sessions", label: vi.tabs.sessions },
];

export function TopBar({ tab, onTab, quota, now, demo, needsYou, onRefresh, onSettings }: Props) {
  return (
    <header className="topbar">
      <div className="brand">
        <svg viewBox="-12 -12 24 24" width={20} height={20} aria-hidden="true">
          <circle r={4} className="brand-star" />
          <circle r={8} className="brand-orbit" />
          <circle cx={8} r={2} className="brand-planet" />
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
          </button>
        ))}
      </nav>

      <div className="topbar-spacer" />

      {demo && (
        <span className="chip chip--demo" title={vi.app.demoHint}>
          {vi.app.demo}
        </span>
      )}
      <QuotaGauges quota={quota} now={now} onRefresh={onRefresh} />
      <span className="kbd-hint" title={vi.topbar.searchHint}>
        <kbd>{vi.topbar.search}</kbd>
      </span>
      <button className="btn btn--icon btn--ghost" aria-label={vi.topbar.notifications} title={vi.topbar.notifications}>
        <svg viewBox="0 0 24 24" width={16} height={16} aria-hidden="true">
          <path d="M6 16V11a6 6 0 1 1 12 0v5l1.5 2h-15z M10 20a2 2 0 0 0 4 0" className="icon-stroke" />
        </svg>
        {needsYou && <span className="bell-dot" />}
      </button>
      <button
        className="btn btn--icon btn--ghost"
        aria-label={vi.topbar.settings}
        title={vi.topbar.settings}
        onClick={onSettings}
      >
        <svg viewBox="0 0 24 24" width={16} height={16} aria-hidden="true">
          <circle cx={12} cy={12} r={3} className="icon-stroke" />
          <path
            d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"
            className="icon-stroke"
          />
        </svg>
      </button>
    </header>
  );
}
