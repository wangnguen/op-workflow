import { describe, expect, it } from "vitest";
import type { RepoState } from "../../ipc/types";
import { branchParts, pickDefaultBase, splitBranches, trackText } from "./startWork";

function repo(names: string[], lastBase?: string): RepoState {
  return {
    path: "C:\\code\\web",
    name: "web",
    branches: names.map((name) => ({ name, remote: name.startsWith("origin/") })),
    dirty: [],
    dirtyCount: 0,
    lastBase,
  };
}

describe("pickDefaultBase", () => {
  it("prefers the remote copy of the task branch, then last base, then usual names", () => {
    const r = repo(["feature/x", "origin/f/WP-1", "origin/main", "origin/development"], "origin/main");
    expect(pickDefaultBase(r, "f/WP-1")).toBe("origin/f/WP-1");
    expect(pickDefaultBase(r, "f/WP-2")).toBe("origin/main");
    expect(pickDefaultBase(repo(["feature/x", "origin/main", "origin/development"]), "f/WP-2")).toBe(
      "origin/development",
    );
  });

  it("ignores a stale last base and falls back to the newest branch", () => {
    expect(pickDefaultBase(repo(["feature/x", "release/1"], "origin/gone"), "f/WP-2")).toBe("feature/x");
    expect(pickDefaultBase(repo([]), "f/WP-2")).toBe("");
  });

  it("does not pick the remote copy when the branch already exists locally", () => {
    expect(pickDefaultBase(repo(["f/WP-1", "origin/f/WP-1", "origin/main"]), "f/WP-1")).toBe("origin/main");
  });
});

describe("branch list helpers", () => {
  it("splits local and remote and filters both", () => {
    const r = repo(["main", "f/WP-1", "origin/main", "origin/f/WP-2"]);
    const all = splitBranches(r.branches, "");
    expect(all.local.map((b) => b.name)).toEqual(["main", "f/WP-1"]);
    expect(all.remote.map((b) => b.name)).toEqual(["origin/main", "origin/f/WP-2"]);
    const wp = splitBranches(r.branches, " WP ");
    expect(wp.local.map((b) => b.name)).toEqual(["f/WP-1"]);
    expect(wp.remote.map((b) => b.name)).toEqual(["origin/f/WP-2"]);
  });

  it("dims the folder part and shortens ahead/behind", () => {
    expect(branchParts("origin/feature/abc")).toEqual({ prefix: "origin/feature/", leaf: "abc" });
    expect(branchParts("main")).toEqual({ prefix: "", leaf: "main" });
    expect(trackText("ahead 2, behind 14")).toBe("↑2 ↓14");
    expect(trackText("behind 3")).toBe("↓3");
    expect(trackText("gone")).toBe("gone");
    expect(trackText(undefined)).toBe("");
  });
});
