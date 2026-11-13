import type { KeyboardEvent } from "react";
import { vi } from "../../i18n/vi";
import { THEMES, type Mode, type ModeSetting, type ThemeMeta } from "../../shared/theme";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { useTheme } from "../../shared/useTheme";

const t = vi.settings;
const STARS: [number, number][] = [
  [8, 14],
  [26, 70],
  [44, 22],
  [62, 78],
  [80, 12],
  [92, 58],
];

/** A tiny window in the theme's colours: top bar, side column, two cards, accent button. */
function Preview({ theme }: { theme: ThemeMeta }) {
  const [bg, panel, accent, text] = theme.swatch;
  return (
    <span className="tp-preview" style={{ background: bg }} aria-hidden="true">
      {theme.id === "galaxy" &&
        STARS.map(([x, y]) => <i key={`${x}-${y}`} className="tp-star" style={{ left: `${x}%`, top: `${y}%` }} />)}
      <span className="tp-bar" style={{ background: panel }}>
        <i style={{ background: accent }} />
      </span>
      <span className="tp-side" style={{ background: panel }} />
      <span className="tp-card-a" style={{ background: panel }}>
        <i style={{ background: text }} />
        <i style={{ background: text, opacity: 0.45 }} />
      </span>
      <span className="tp-card-b" style={{ background: panel }}>
        <i style={{ background: text, opacity: 0.6 }} />
        <b style={{ background: accent }} />
      </span>
    </span>
  );
}

function Group({ mode }: { mode: Mode }) {
  const { prefs, theme, pick } = useTheme();
  const list = THEMES.filter((x) => x.mode === mode);
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (step === undefined) return;
    e.preventDefault();
    const at = list.findIndex((x) => x.id === prefs[mode]);
    const next = list[(at + step + list.length) % list.length];
    if (!next) return;
    pick(next.id);
    e.currentTarget.querySelector<HTMLElement>(`[data-theme-option="${next.id}"]`)?.focus();
  };
  return (
    <section className="tp-group">
      <p className="label">{mode === "dark" ? t.themeDark : t.themeLight}</p>
      <div className="tp-grid" role="radiogroup" aria-label={mode === "dark" ? t.themeDark : t.themeLight} onKeyDown={onKey}>
        {list.map((x) => {
          const chosen = prefs[mode] === x.id;
          return (
            <button
              key={x.id}
              type="button"
              role="radio"
              aria-checked={chosen}
              tabIndex={chosen ? 0 : -1}
              data-theme-option={x.id}
              className={`tp-card${chosen ? " is-on" : ""}${theme.id === x.id ? " is-shown" : ""}`}
              onClick={() => pick(x.id)}
            >
              <Preview theme={x} />
              <span className="tp-name">
                <span>{x.name}</span>
                {chosen && <Icon name="check" size={13} />}
              </span>
              <span className="tp-desc">{x.description}</span>
            </button>
          );
        })}
      </div>
    </section>
  );
}

/** Cài đặt → Giao diện: light / dark / follow Windows, and one theme per mode. */
export function ThemePicker() {
  const { prefs, mode, setMode } = useTheme();
  // the mode on screen first
  const order: Mode[] = mode === "light" ? ["light", "dark"] : ["dark", "light"];
  return (
    <div className="tp">
      <SegmentedControl<ModeSetting>
        label={t.themeMode}
        value={prefs.mode}
        onChange={setMode}
        options={[
          { value: "light", label: t.modeLight, icon: <Icon name="sun" size={13} /> },
          { value: "dark", label: t.modeDark, icon: <Icon name="moon" size={13} /> },
          { value: "system", label: t.modeSystem, icon: <Icon name="laptop" size={13} />, title: t.modeSystemHint },
        ]}
      />
      {order.map((m) => (
        <Group key={m} mode={m} />
      ))}
      <p className="set-note">{t.themeNote}</p>
    </div>
  );
}
