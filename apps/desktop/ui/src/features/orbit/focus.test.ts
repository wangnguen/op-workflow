import { describe, expect, it } from "vitest";
import { orbitForFrame } from "../../ipc/mock";
import { focusSystem, systemsOf } from "./focus";

const NOW = Date.parse("2026-11-13T08:00:00Z");

describe("focusSystem", () => {
  it("swaps the picked task into the centre and keeps the rest", () => {
    const s = orbitForFrame(3, NOW);
    const ids = systemsOf(s).map((x) => x.taskId);
    expect(ids.length).toBeGreaterThan(1);
    const second = ids[1] ?? "";
    const f = focusSystem(s, second);
    expect(f.active?.taskId).toBe(second);
    expect(systemsOf(f).map((x) => x.taskId).sort()).toEqual([...ids].sort());
    expect(focusSystem(s, "WP-unknown")).toBe(s);
    expect(focusSystem(s, null)).toBe(s);
  });
});
