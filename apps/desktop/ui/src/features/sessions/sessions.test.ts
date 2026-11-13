import { describe, expect, it } from "vitest";
import { orbitForFrame } from "../../ipc/mock";
import { groupSessions } from "./sessions";

const NOW = Date.parse("2026-11-13T08:00:00Z");

describe("groupSessions", () => {
  it("puts the main agent under Cần bạn while a gate is open", () => {
    const g = groupSessions(orbitForFrame(2, NOW));
    expect(g.waiting.some((s) => s.id === "claude-main")).toBe(true);
  });

  it("includes child agents and outside sessions", () => {
    const g = groupSessions(orbitForFrame(10, NOW));
    const all = [...g.waiting, ...g.running, ...g.idle];
    expect(all.some((s) => s.agent === "codex" && s.inApp)).toBe(true);
    expect(all.filter((s) => !s.inApp)).toHaveLength(3);
  });
});
