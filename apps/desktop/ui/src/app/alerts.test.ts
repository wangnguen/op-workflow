import { describe, expect, it } from "vitest";
import type { AlertView } from "../ipc/types";
import { visibleAlerts } from "./alerts";

describe("bell active and deleted lists", () => {
  it("excludes deleted items from All and Unread, newest first, and orders Deleted by deletion time", () => {
    const item = (id: number, read: boolean, deletedAt?: string): AlertView => ({
      id, at: "2026-11-01T00:00:00Z", kind: "quota", title: "quota", body: "", read, deletedAt,
    });
    const items = [item(1, false), item(2, true), item(3, false, "2026-11-17T00:00:00Z"), item(4, true, "2026-11-18T00:00:00Z")];
    expect(visibleAlerts(items, "unread").map((a) => a.id)).toEqual([1]);
    expect(visibleAlerts(items, "all").map((a) => a.id)).toEqual([2, 1]);
    expect(visibleAlerts(items, "deleted").map((a) => a.id)).toEqual([4, 3]);
    expect(items.map((a) => a.id)).toEqual([1, 2, 3, 4]);
  });
});
