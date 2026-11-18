import { describe, expect, it } from "vitest";
import type { TestView } from "../../ipc/types";
import { retestNote, testOutcome, testTally } from "./TestsSection";

const view = (step: TestView["step"], passed: boolean, cases: TestView["cases"]): TestView => ({
  step,
  passed,
  summary: "",
  cases,
  at: "2026-11-18T06:00:00Z",
  runs: 1,
});

describe("test card", () => {
  it("counts only the cases that ran, and says how many could not be tested", () => {
    const partial = view("browserTest", false, [
      { name: "1. Danh sách", ok: true },
      { name: "4. Xuất Excel", ok: false, untested: true, detail: "không bắt được file" },
      { name: "6. Xoá hàng loạt", ok: false, untested: true },
    ]);
    expect(testOutcome(partial)).toBe("partial");
    expect(testTally(partial)).toBe("OK 1/1 · 2 chưa test");
    expect(retestNote(partial)).toBe(
      "Chỉ test các ca lần trước chưa test được: 4. Xuất Excel; 6. Xoá hàng loạt. Các ca khác đã xong, không cần test lại.",
    );
    const ng = view("browserTest", false, [{ name: "1", ok: false }, ...partial.cases.slice(1)]);
    expect(testOutcome(ng)).toBe("fail");
    expect(testTally(ng)).toBe("NG 0/1 · 2 chưa test");
    expect(testTally(view("browserTest", false, [{ name: "x", ok: false, untested: true }]))).toBe("Chưa test được ca nào");
  });

  it("keeps PASS / FAIL for the harness", () => {
    expect(testTally(view("harness", true, [{ name: "a", ok: true }]))).toBe("PASS 1/1");
    expect(testTally(view("harness", false, [{ name: "a", ok: false }]))).toBe("FAIL 0/1");
    expect(testOutcome(view("harness", true, []))).toBe("pass");
  });
});
