import { describe, it, expect } from "vitest";
import { springs, durations, variants } from "./motion";

describe("motion tokens", () => {
  it("exposes spring presets as spring transitions", () => {
    expect(springs.snappy.type).toBe("spring");
    expect(springs.cable.type).toBe("spring");
  });

  it("exposes ordered durations", () => {
    expect(durations.fast).toBeLessThan(durations.base);
    expect(durations.base).toBeLessThan(durations.slow);
  });

  it("stagger parent orchestrates children", () => {
    const show = variants.listStagger.show as { transition?: { staggerChildren?: number } };
    expect(show.transition?.staggerChildren).toBeGreaterThan(0);
  });

  it("slide direction flips x sign", () => {
    const right = variants.slide(1).enter as { x: number };
    const left = variants.slide(-1).enter as { x: number };
    expect(Math.sign(right.x)).toBe(1);
    expect(Math.sign(left.x)).toBe(-1);
  });
});
