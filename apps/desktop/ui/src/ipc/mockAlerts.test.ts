import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

beforeEach(() => { vi.resetModules(); vi.useFakeTimers(); vi.setSystemTime(new Date("2026-11-17T00:00:00Z")); });
afterEach(() => { vi.useRealTimers(); });

describe("notification deletion in the preview", () => {
  it("deletes selected and all active alerts, and restores them as read", async () => {
    const api = await import("./mockTask");
    expect(api.mockUnreadAlerts()).toBe(2);
    const original = api.mockAlerts();
    api.mockDeleteAlerts([3]);
    expect(api.mockUnreadAlerts()).toBe(1);
    expect(api.mockAlerts().find((a) => a.id === 3)).toMatchObject({ read: true, deletedAt: "2026-11-17T00:00:00.000Z" });
    api.mockRestoreAlerts([3]);
    expect(api.mockUnreadAlerts()).toBe(1);
    expect(api.mockAlerts().find((a) => a.id === 3)?.deletedAt).toBeUndefined();
    api.mockDeleteAlerts([]);
    expect(api.mockUnreadAlerts()).toBe(0);
    expect(api.mockAlerts().every((a) => a.deletedAt)).toBe(true);
    api.mockRestoreAlerts([]);
    expect(api.mockAlerts().every((a) => a.read && !a.deletedAt)).toBe(true);
    expect(original[0]?.deletedAt).toBeUndefined(); // callers cannot mutate the stored list
  });

  it("expires only deleted alerts at 30 days, measured from deletion", async () => {
    const api = await import("./mockTask");
    const day = 24 * 60 * 60 * 1000;
    api.mockDeleteAlerts([3]);
    vi.setSystemTime(Date.now() + 29 * day);
    expect(api.mockAlerts().find((a) => a.id === 3)?.deletedAt).toBeDefined();
    api.mockDeleteAlerts([3]); // repeated delete does not extend the deadline
    vi.setSystemTime(Date.now() + day);
    expect(api.mockAlerts().map((a) => a.id)).toEqual([2, 1]);
    api.mockRestoreAlerts([3]);
    expect(api.mockAlerts().map((a) => a.id)).toEqual([2, 1]);
  });
});
