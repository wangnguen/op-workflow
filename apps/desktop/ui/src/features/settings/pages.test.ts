import { describe, expect, it } from "vitest";
import { fold, searchEntries } from "./pages";

describe("settings search", () => {
  it("ignores Vietnamese accents and case", () => {
    expect(fold("Phiên bản ĐÃ")).toBe("phien ban da");
    expect(searchEntries("phien ban").map((e) => e.id)).toContain("version");
    expect(searchEntries("TOKEN").map((e) => e.id)).toEqual(["token"]);
  });

  it("matches English keywords and needs every word", () => {
    expect(searchEntries("update").map((e) => e.id)).toEqual(["version"]);
    expect(searchEntries("quota codex").map((e) => e.id)).toEqual(["codex"]);
    expect(searchEntries("   ")).toEqual([]);
    expect(searchEntries("khong-co-gi")).toEqual([]);
  });
});
