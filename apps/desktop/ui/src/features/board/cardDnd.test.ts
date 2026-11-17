import { describe, expect, it } from "vitest";
import { nextColumnCenter, type ColumnBox } from "./cardDnd";

// Inbox, Đang làm, Chờ push 300 px wide, then the folded "Đã xong" strip (46 px), out of order
const cols: ColumnBox[] = [
  { id: "done", left: 960, width: 46 },
  { id: "inbox", left: 0, width: 300 },
  { id: "awaitingPush", left: 640, width: 300 },
  { id: "active", left: 320, width: 300 },
];

describe("moving a lifted card with the arrow keys", () => {
  it("jumps to the centre of the next or previous column", () => {
    expect(nextColumnCenter(cols, 150, 1)).toBe(470);
    expect(nextColumnCenter(cols, 470, -1)).toBe(150);
    // onto the narrow folded strip
    expect(nextColumnCenter(cols, 790, 1)).toBe(983);
  });

  it("follows the column the card is over, and stops at the ends", () => {
    expect(nextColumnCenter(cols, 150, 1, "active")).toBe(790);
    expect(nextColumnCenter(cols, 150, -1)).toBeUndefined();
    expect(nextColumnCenter(cols, 983, 1)).toBeUndefined();
    // in the gap between two columns and over none
    expect(nextColumnCenter(cols, 310, 1)).toBeUndefined();
  });
});
