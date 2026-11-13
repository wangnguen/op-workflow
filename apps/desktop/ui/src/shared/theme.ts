// Colour themes: `:root[data-theme=<id>]` in styles/themes.css ("galaxy" is tokens.css itself)
// plus `data-mode` (light/dark). Each mode remembers its own theme; "system" follows Windows.

export type Mode = "light" | "dark";
export type ModeSetting = Mode | "system";

export interface ThemeMeta {
  id: string;
  name: string;
  mode: Mode;
  description: string;
  /** preview: background, panel, accent, text */
  swatch: [string, string, string, string];
}

export const THEMES: readonly ThemeMeta[] = [
  { id: "galaxy", name: "Ngân hà", mode: "dark", description: "Mặc định: nền xanh đen, điểm nhấn cam", swatch: ["#06070c", "#10121b", "#f2994a", "#e9ebf3"] },
  { id: "graphite", name: "Than chì", mode: "dark", description: "Xám trung tính, điểm nhấn cam đất", swatch: ["#131315", "#1b1b1f", "#e8835d", "#ececf1"] },
  { id: "dracula", name: "Dracula", mode: "dark", description: "Tím than, tím nhạt và hồng", swatch: ["#1e1f29", "#282a36", "#bd93f9", "#f8f8f2"] },
  { id: "tokyo-night", name: "Tokyo Night", mode: "dark", description: "Xanh đêm, chữ xanh lam nhạt", swatch: ["#16161e", "#1f2335", "#7aa2f7", "#c0caf5"] },
  { id: "catppuccin-mocha", name: "Catppuccin Mocha", mode: "dark", description: "Pastel trên nền tím đen", swatch: ["#11111b", "#1e1e2e", "#cba6f7", "#cdd6f4"] },
  { id: "nord", name: "Nord", mode: "dark", description: "Xám xanh Bắc Âu, điểm nhấn băng", swatch: ["#242933", "#2e3440", "#88c0d0", "#eceff4"] },
  { id: "gruvbox-dark", name: "Gruvbox", mode: "dark", description: "Ấm, kiểu retro, điểm nhấn cam", swatch: ["#1d2021", "#282828", "#fe8019", "#ebdbb2"] },
  { id: "rose-pine", name: "Rosé Pine", mode: "dark", description: "Tím dịu, điểm nhấn hồng phấn", swatch: ["#191724", "#1f1d2e", "#ebbcba", "#e0def4"] },
  { id: "one-dark", name: "One Dark", mode: "dark", description: "Xám xanh kiểu editor, xanh da trời", swatch: ["#21252b", "#282c34", "#61afef", "#e6e9ef"] },
  { id: "solarized-dark", name: "Solarized Dark", mode: "dark", description: "Xanh biển sâu, chữ kem, ít chói", swatch: ["#00212b", "#002b36", "#268bd2", "#eee8d5"] },
  { id: "monokai", name: "Monokai", mode: "dark", description: "Nâu đen, điểm nhấn xanh chanh", swatch: ["#1e1f1c", "#272822", "#a6e22e", "#f8f8f2"] },
  { id: "paper", name: "Giấy", mode: "light", description: "Mặc định: trắng xám, điểm nhấn cam đất", swatch: ["#ececef", "#ffffff", "#bd5128", "#1d1d1f"] },
  { id: "catppuccin-latte", name: "Catppuccin Latte", mode: "light", description: "Pastel sáng, điểm nhấn tím", swatch: ["#e6e9ef", "#eff1f5", "#8839ef", "#4c4f69"] },
  { id: "tokyo-day", name: "Tokyo Night Day", mode: "light", description: "Xám xanh sáng, điểm nhấn xanh dương", swatch: ["#d5d6db", "#e9e9ed", "#2e7de9", "#343b58"] },
  { id: "rose-pine-dawn", name: "Rosé Pine Dawn", mode: "light", description: "Kem hồng, điểm nhấn xanh thông", swatch: ["#f2e9e1", "#fffaf3", "#286983", "#575279"] },
  { id: "gruvbox-light", name: "Gruvbox Light", mode: "light", description: "Giấy vàng cũ, điểm nhấn cam gạch", swatch: ["#f2e5bc", "#fbf1c7", "#af3a03", "#3c3836"] },
  { id: "one-light", name: "One Light", mode: "light", description: "Trắng xám, điểm nhấn xanh", swatch: ["#eaeaeb", "#fafafa", "#4078f2", "#383a42"] },
  { id: "solarized-light", name: "Solarized Light", mode: "light", description: "Nền kem, chữ xanh đậm", swatch: ["#eee8d5", "#fdf6e3", "#268bd2", "#073642"] },
];

const byId = new Map(THEMES.map((t) => [t.id, t]));
export const DEFAULT_THEME: Record<Mode, string> = { dark: "galaxy", light: "paper" };

export interface ThemePrefs {
  mode: ModeSetting;
  /** theme used in dark mode */
  dark: string;
  /** theme used in light mode */
  light: string;
}

export const DEFAULT_PREFS: ThemePrefs = { mode: "dark", dark: DEFAULT_THEME.dark, light: DEFAULT_THEME.light };
const KEY = "opw.theme.v2";

export function themeMeta(id: string, mode: Mode): ThemeMeta {
  const t = byId.get(id);
  return t && t.mode === mode ? t : (byId.get(DEFAULT_THEME[mode]) as ThemeMeta);
}

/** Saved choice; storage can be unavailable (private window, cleared data). */
export function loadPrefs(): ThemePrefs {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (!raw || typeof raw !== "object") return DEFAULT_PREFS;
    const p = raw as Partial<ThemePrefs>;
    return {
      mode: p.mode === "light" || p.mode === "dark" || p.mode === "system" ? p.mode : DEFAULT_PREFS.mode,
      dark: themeMeta(String(p.dark), "dark").id,
      light: themeMeta(String(p.light), "light").id,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

export function savePrefs(p: ThemePrefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // still applies for this session
  }
}

export function systemMode(): Mode {
  if (typeof window === "undefined" || !window.matchMedia) return "dark";
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function effectiveMode(p: ThemePrefs, system: Mode): Mode {
  return p.mode === "system" ? system : p.mode;
}

/** The theme to show for these prefs. */
export function resolveTheme(p: ThemePrefs, system: Mode): ThemeMeta {
  const mode = effectiveMode(p, system);
  return themeMeta(p[mode], mode);
}

export function applyTheme(t: ThemeMeta): void {
  const root = document.documentElement;
  if (t.id === DEFAULT_THEME.dark) delete root.dataset.theme;
  else root.dataset.theme = t.id;
  root.dataset.mode = t.mode;
}

/** Picking a theme also switches to its mode, unless the mode follows Windows. */
export function pickTheme(p: ThemePrefs, id: string): ThemePrefs {
  const t = byId.get(id);
  if (!t) return p;
  return { ...p, [t.mode]: t.id, mode: p.mode === "system" ? "system" : t.mode };
}
