import { describe, expect, it } from "vitest";
import { textUnits, truncate, wrapText } from "./text";

describe("svg text helpers", () => {
  it("truncates only long text", () => {
    expect(truncate("ngắn", 10)).toBe("ngắn");
    expect(truncate("Lỗi đồng bộ quyền khi đổi owner", 10)).toBe("Lỗi đồng …");
  });

  it("wraps at spaces and keeps every character", () => {
    const text = "Lỗi đồng bộ quyền khi đổi owner thư mục chia sẻ";
    const lines = wrapText(text, 16);
    expect(lines.every((l) => l.length <= 16)).toBe(true);
    expect(lines.join(" ")).toBe(text);
    expect(wrapText("a-very-long-identifier-without-spaces", 10)).toEqual([
      "a-very-lon",
      "g-identifi",
      "er-without",
      "-spaces",
    ]);
    expect(wrapText("   ", 5)).toEqual([]);
  });

  it("counts CJK and full-width characters as two units", () => {
    expect(textUnits("abc")).toBe(3);
    expect(textUnits("権限")).toBe(4);
    expect(textUnits("■ x")).toBe(4);
    expect(truncate("テスト仕様書の確認", 8)).toBe("テスト…");
  });

  /** The title that overflowed its card on the map (2026-11-14). */
  it("keeps a Japanese title inside the card width", () => {
    const title =
      "■ RightClicker ■ [Bug] Groupのコラボレーション権限変更で403 insufficient_scopeエラーが発生する問題の調査・対応 / Điều tra và khắc phục lỗi 403";
    const lines = wrapText(title, 38);
    expect(lines.every((l) => textUnits(l) <= 38)).toBe(true);
    expect(lines.join("").replace(/\s/g, "")).toBe(title.replace(/\s/g, ""));
    // breaks between kana instead of cutting "insufficient_scope" apart
    expect(lines.some((l) => l.startsWith("insufficient_scope"))).toBe(true);
  });
});
