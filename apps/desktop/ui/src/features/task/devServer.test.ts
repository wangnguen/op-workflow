import { describe, expect, it } from "vitest";
import { urlProblem } from "./DevServerBox";

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
