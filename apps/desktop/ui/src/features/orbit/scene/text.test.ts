import { describe, expect, it } from "vitest";
import { truncate, wrapText } from "./text";

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
});
