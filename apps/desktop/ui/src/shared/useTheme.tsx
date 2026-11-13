import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  DEFAULT_PREFS,
  applyTheme,
  loadPrefs,
  pickTheme,
  resolveTheme,
  savePrefs,
  systemMode,
  type Mode,
  type ModeSetting,
  type ThemeMeta,
  type ThemePrefs,
} from "./theme";

interface ThemeState {
  prefs: ThemePrefs;
  /** theme on screen now */
  theme: ThemeMeta;
  mode: Mode;
  setMode: (mode: ModeSetting) => void;
  pick: (id: string) => void;
}

const ThemeContext = createContext<ThemeState>({
  prefs: DEFAULT_PREFS,
  theme: resolveTheme(DEFAULT_PREFS, "dark"),
  mode: "dark",
  setMode: () => {},
  pick: () => {},
});

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [prefs, setPrefs] = useState<ThemePrefs>(loadPrefs);
  const [system, setSystem] = useState<Mode>(systemMode);

  // Windows switched light/dark while the app is open
  useEffect(() => {
    if (!window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const on = () => setSystem(mq.matches ? "light" : "dark");
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);

  const theme = resolveTheme(prefs, system);
  useEffect(() => applyTheme(theme), [theme]);

  const update = useCallback((next: (p: ThemePrefs) => ThemePrefs) => {
    setPrefs((p) => {
      const n = next(p);
      savePrefs(n);
      return n;
    });
  }, []);

  const value = useMemo<ThemeState>(
    () => ({
      prefs,
      theme,
      mode: theme.mode,
      setMode: (mode) => update((p) => ({ ...p, mode })),
      pick: (id) => update((p) => pickTheme(p, id)),
    }),
    [prefs, theme, update],
  );
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export const useTheme = (): ThemeState => useContext(ThemeContext);
