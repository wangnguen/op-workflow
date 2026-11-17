import { describe, expect, it } from "vitest";
import { NEBULA_DEFAULT, nebulaTone } from "./nebulaTone";

describe("nebulaTone", () => {
  it("uses the OpenProject colour of the priority, stronger the higher it ranks", () => {
    const low = nebulaTone({ name: "Low", rank: 1, ranks: 4 });
    const high = nebulaTone({ name: "High", color: "#FF922B", rank: 3, ranks: 4 });
    const top = nebulaTone({ name: "Immediate", color: "#e03131", rank: 4, ranks: 4 });
    expect(high.color).toBe("#ff922b");
    expect(low.color).toBe(NEBULA_DEFAULT);
    expect(low.weight).toBe(0);
    expect(top.weight).toBe(1);
    expect(low.cloud).toBeLessThan(high.cloud);
    expect(high.cloud).toBeLessThan(top.cloud);
    expect(low.core).toBeLessThan(top.core);
  });

  it("sits in the middle when the order or the priority is unknown", () => {
    expect(nebulaTone(undefined)).toMatchObject({ color: NEBULA_DEFAULT, weight: 0.5 });
    expect(nebulaTone({ name: "Normal", color: "red" })).toMatchObject({ color: NEBULA_DEFAULT, weight: 0.5 });
    expect(nebulaTone({ name: "Odd", rank: 5, ranks: 4 }).weight).toBe(0.5);
  });
});
