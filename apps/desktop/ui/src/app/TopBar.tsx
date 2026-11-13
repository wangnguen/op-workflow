import { vi } from "../i18n/vi";
import type { ProviderLink, QuotaSnapshot, UpdateView } from "../ipc/types";
import { QuotaGauges } from "../features/quota/QuotaGauges";

export type Tab = "task" | "workspace" | "sessions";

interface Props {
  tab: Tab;
  onTab: (t: Tab) => void;
  quota: QuotaSnapshot[];
  links: ProviderLink[];
  now: number;
  demo: boolean;
  needsYou: boolean;
  update: UpdateView | undefined;
  onRefresh: () => void;
  onSettings: () => void;
  onOpenRelease: () => void;
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
  update,
  onRefresh,
  onSettings,
  onOpenRelease,
}: Props) {
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
          <path d={GEAR_PATH} className="icon-stroke" />
          <circle cx={12} cy={12} r={3} className="icon-stroke" />
        </svg>
        {update?.available && <span className="bell-dot bell-dot--update" />}
      </button>
    </header>
  );
}
