import { describe, expect, it } from "vitest";
import { chipColors, statusColor } from "./statusColor";

describe("status colours", () => {
  it("uses the OpenProject colour, else a stable fallback per name", () => {
    expect(statusColor("In progress", "#CC5DE8")).toBe("#cc5de8");
    expect(statusColor("Weird", "red")).toBe(statusColor("weird", undefined));
    expect(statusColor("Open", undefined)).toBe(statusColor(" open ", undefined));
  });

  it("lifts dark colours so the text stays readable", () => {
    const black = chipColors("To Do", "#000000");
    expect(black.ink).toMatch(/ 70%\)$/);
    expect(black.dot).toMatch(/ 55%\)$/);
    const pale = chipColors("Cancelled", "#f3f0ff");
    expect(pale.ink).toMatch(/ 85%\)$/);
    expect(chipColors("Open", "#1a67a3").dot).toBe("#1a67a3");
    // light UI: pale colours get darker text
    expect(chipColors("Cancelled", "#f3f0ff", "light").ink).toMatch(/ 36%\)$/);
    expect(chipColors("Open", "#1a67a3", "light").ink).toMatch(/ 36%\)$/);
  });
});
