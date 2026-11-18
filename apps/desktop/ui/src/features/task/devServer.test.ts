import { describe, expect, it } from "vitest";
import { MAX_HOW_TO, serverProblem, tidy, urlProblem, waitOk } from "./DevServerBox";

describe("dev server commands", () => {
  const api = { command: "npm run dev", dir: "api", wait: "3000" };
  const web = { command: "npm run dev", dir: "web", wait: "" };
  const url = "http://localhost:3003";

  it("waits for a port or a URL", () => {
    for (const ok of ["3000", ":3000", "http://localhost:3000", "http://127.0.0.1:3000/health"]) expect(waitOk(ok)).toBe(true);
    for (const bad of ["", "0", "70000", "localhost:3000", "api"]) expect(waitOk(bad)).toBe(false);
  });

  it("takes one command, several, or none (already running)", () => {
    expect(serverProblem({ url, commands: [web] })).toBeUndefined();
    expect(serverProblem({ url, commands: [api, web] })).toBeUndefined();
    expect(serverProblem({ url, commands: [] })).toBeUndefined();
    // a row left empty is ignored, also when it is the last one
    expect(serverProblem({ url, commands: [api, web, { command: "", dir: "", wait: "" }] })).toBeUndefined();
  });

  it("says what to fix, numbered as on screen", () => {
    expect(serverProblem({ url, commands: [api, { ...web, command: "" , wait: "3003" }] })).toBe("Lệnh 2 chưa có lệnh");
    expect(serverProblem({ url, commands: [{ ...api, wait: "api" }, web] })).toMatch(/^Lệnh 1: ô Chờ/);
    // only the last command may wait for the URL
    expect(serverProblem({ url, commands: [web, api] })).toMatch(/^Lệnh 1: ghi cổng chờ/);
    expect(serverProblem({ url: "", commands: [web] })).toBe("Có lệnh chạy thì cần URL của app");
    expect(serverProblem({ url: "localhost:3003", commands: [web] })).toBe("URL phải dạng http://localhost:5173");
  });

  it("saves trimmed rows, empty ones dropped", () => {
    expect(
      tidy({ url: ` ${url} `, commands: [{ command: " a ", dir: " api ", wait: " 3000 " }, { command: "", dir: "x", wait: "" }, web] }),
    ).toEqual({ url, commands: [{ command: "a", dir: "api", wait: "3000" }, web], howTo: "" });
  });

  it("keeps how to reach the screen, also without a URL, within its length", () => {
    expect(tidy({ url: "", commands: [], howTo: "  bấm … → 統合 " })).toEqual({ url: "", commands: [], howTo: "bấm … → 統合" });
    expect(serverProblem({ url: "", commands: [], howTo: "thư mục test 4348" })).toBeUndefined();
    expect(serverProblem({ url, commands: [web], howTo: "x".repeat(MAX_HOW_TO + 1) })).toBe(`Cách vào chỗ test tối đa ${MAX_HOW_TO} ký tự`);
  });
});

describe("dev server URL", () => {
  it("takes an http(s) URL with a host, or nothing", () => {
    expect(urlProblem("")).toBeUndefined();
    expect(urlProblem("http://localhost:5173")).toBeUndefined();
    expect(urlProblem("https://app.example.com/files?x=1")).toBeUndefined();
    expect(urlProblem("localhost:5173")).toBe("URL phải dạng http://localhost:5173");
    expect(urlProblem("http://")).toBeDefined();
    expect(urlProblem("http://local host")).toBeDefined();
  });
});
