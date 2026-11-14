import { describe, expect, it } from "vitest";
import type { DiffFile, DiffView, FindingView } from "../../ipc/types";
import { findingsByLine, normPath, outsideFindings, samePath, splitRows } from "./diff";

const file: DiffFile = {
  path: "api/src/routes/files.ts",
  change: "modified",
  binary: false,
  untracked: false,
  added: 2,
  removed: 1,
  truncated: false,
  changed: false,
  hunks: [
    {
      section: "",
      lines: [
        { kind: "context", old: 40, new: 40, text: "a" },
        { kind: "removed", old: 41, text: "b" },
        { kind: "added", new: 41, text: "c" },
        { kind: "added", new: 42, text: "d" },
        { kind: "context", old: 42, new: 43, text: "e" },
      ],
    },
  ],
};

const finding = (over: Partial<FindingView>): FindingView => ({
  fingerprint: "f",
  severity: "high",
  title: "t",
  file: "api/src/routes/files.ts",
  line: 41,
  source: "codex",
  state: "open",
  ...over,
});

describe("diff helpers", () => {
  it("compares reviewer paths with git paths", () => {
    expect(normPath(".\\api\\a.ts")).toBe("api/a.ts");
    expect(samePath("C:/code/acme-api/api/src/routes/files.ts", file.path)).toBe(true);
    expect(samePath("src/routes/files.ts", file.path)).toBe(true);
    expect(samePath("api/src/routes/file.ts", file.path)).toBe(false);
  });

  it("puts open findings on their line and lists the rest as outside", () => {
    const list = [
      finding({ fingerprint: "on" }),
      finding({ fingerprint: "fixed", state: "fixed" }),
      finding({ fingerprint: "far", line: 99 }),
      finding({ fingerprint: "other", file: "web/x.ts" }),
    ];
    const byLine = findingsByLine(list, file);
    expect(byLine.get(41)?.map((f) => f.fingerprint)).toEqual(["on"]);
    expect(byLine.get(99)?.map((f) => f.fingerprint)).toEqual(["far"]);
    const diff = { files: [file] } as DiffView;
    expect(outsideFindings(list, diff).map((f) => f.fingerprint)).toEqual(["far", "other"]);
  });

  it("pairs removed and added runs side by side", () => {
    const rows = splitRows(file.hunks[0]!);
    expect(rows.map((r) => [r.left?.text, r.right?.text])).toEqual([
      ["a", "a"],
      ["b", "c"],
      [undefined, "d"],
      ["e", "e"],
    ]);
  });
});
