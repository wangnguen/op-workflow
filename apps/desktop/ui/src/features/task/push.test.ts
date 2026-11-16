import { describe, expect, it } from "vitest";
import { mockTaskDetail } from "../../ipc/mockFlow";
import type { PushView } from "../../ipc/types";
import { pushParts } from "./push";

const view = (blocks: [string, string][]): PushView => ({
  text: "",
  blocks: blocks.map(([label, text]) => ({ label, text })),
});

describe("push summary parts", () => {
  it("finds the commit message and the PR description by heading", () => {
    expect(pushParts(view([["PR description", "## Summary"], ["Commit message", "fix: x"]]))).toEqual({
      commit: "fix: x",
      pr: "## Summary",
    });
    expect(pushParts(view([["コミットメッセージ (commit)", "fix"], ["Pull request", "pr"]]))).toEqual({ commit: "fix", pr: "pr" });
  });

  it("falls back to block order, and to nothing", () => {
    expect(pushParts(view([["text", "fix: y"], ["markdown", "body"]]))).toEqual({ commit: "fix: y", pr: "body" });
    expect(pushParts(view([["Nội dung", "all"]]))).toEqual({ commit: "all", pr: undefined });
    expect(pushParts(undefined)).toEqual({ commit: undefined, pr: undefined });
  });

  it("the demo card waiting for the push has both parts", () => {
    const parts = pushParts(mockTaskDetail(186196).push);
    expect(parts.commit).toMatch(/^fix\(files\)/);
    expect(parts.pr).toMatch(/^## Summary/);
  });
});
