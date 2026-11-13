import { describe, expect, it } from "vitest";
import { DEFAULT_PREFS, THEMES, pickTheme, resolveTheme, themeMeta } from "./theme";

describe("themes", () => {
  it("has unique ids and both modes", () => {
    expect(new Set(THEMES.map((t) => t.id)).size).toBe(THEMES.length);
    expect(THEMES.some((t) => t.mode === "light")).toBe(true);
    expect(THEMES.some((t) => t.mode === "dark")).toBe(true);
  });

  it("each mode keeps its own theme; system follows Windows", () => {
    let p = pickTheme(DEFAULT_PREFS, "dracula");
    expect(resolveTheme(p, "light").id).toBe("dracula");
    p = pickTheme(p, "catppuccin-latte");
    expect(p.mode).toBe("light");
    expect(p.dark).toBe("dracula");
    p = { ...p, mode: "system" };
    expect(resolveTheme(p, "dark").id).toBe("dracula");
    expect(resolveTheme(p, "light").id).toBe("catppuccin-latte");
    expect(pickTheme(p, "nord").mode).toBe("system");
  });

  it("falls back to the default of the mode for unknown or wrong-mode ids", () => {
    expect(themeMeta("nope", "dark").id).toBe("galaxy");
    expect(themeMeta("dracula", "light").id).toBe("paper");
  });
});
