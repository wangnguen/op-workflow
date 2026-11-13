import { describe, expect, it } from "vitest";
import {
  clamp,
  formatCountdown,
  formatDuration,
  formatRelative,
  formatTokens,
  formatUsd,
} from "./format";

const NOW = Date.parse("2026-11-13T08:00:00Z");
const ago = (sec: number) => new Date(NOW - sec * 1000).toISOString();

describe("format", () => {
  it("formats tokens", () => {
    expect(formatTokens(950)).toBe("950");
    expect(formatTokens(36_500)).toBe("36.5K");
    expect(formatTokens(1_240_000)).toBe("1.2M");
    expect(formatTokens(500_000)).toBe("500K");
  });

  it("formats money and clamps", () => {
    expect(formatUsd(0.8412)).toBe("$0.84");
    expect(clamp(120, 0, 100)).toBe(100);
    expect(clamp(-3, 0, 100)).toBe(0);
  });

  it("formats durations", () => {
    expect(formatDuration(40_000)).toBe("40 giây");
    expect(formatDuration(12 * 60_000)).toBe("12 phút");
    expect(formatDuration((2 * 60 + 14) * 60_000)).toBe("2 giờ 14 phút");
  });

  it("formats relative time in Vietnamese", () => {
    expect(formatRelative(ago(10), NOW)).toBe("vừa xong");
    expect(formatRelative(ago(5 * 60), NOW)).toBe("5 phút trước");
    expect(formatRelative(ago(3 * 3600), NOW)).toBe("3 giờ trước");
    expect(formatRelative(ago(26 * 3600), NOW)).toBe("hôm qua");
    expect(formatRelative(undefined, NOW)).toBe("—");
  });

  it("formats countdowns and passed resets", () => {
    const later = new Date(NOW + 90 * 60_000).toISOString();
    expect(formatCountdown(later, NOW, "đã reset")).toBe("còn 1 giờ 30 phút");
    expect(formatCountdown(ago(5), NOW, "đã reset")).toBe("đã reset");
  });
});
