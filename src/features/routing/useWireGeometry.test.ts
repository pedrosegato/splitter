import { describe, expect, test } from "vitest";
import { cable, catenaryDroop, sagFor, streamColor } from "./useWireGeometry";

describe("catenaryDroop", () => {
  test("is zero at the anchors and one at the belly", () => {
    expect(catenaryDroop(0)).toBeCloseTo(0);
    expect(catenaryDroop(1)).toBeCloseTo(0);
    expect(catenaryDroop(0.5)).toBeCloseTo(1);
  });

  test("is symmetric around the midpoint", () => {
    expect(catenaryDroop(0.25)).toBeCloseTo(catenaryDroop(0.75));
  });
});

describe("cable", () => {
  test("starts at point a as a polyline", () => {
    const d = cable({ x: 260, y: 100 }, { x: 560, y: 200 }, 40);
    expect(d.startsWith("M260,100 L")).toBe(true);
  });

  function points(d: string): Array<{ x: number; y: number }> {
    return d
      .replace(/[ML]/g, " ")
      .trim()
      .split(/\s+/)
      .map((pair) => {
        const [x, y] = pair.split(",").map(Number);
        return { x, y };
      });
  }

  test("belly droops by ~sag below the straight line", () => {
    const a = { x: 0, y: 0 };
    const b = { x: 100, y: 0 };
    const sag = 50;
    const pts = points(cable(a, b, sag));
    const maxY = Math.max(...pts.map((p) => p.y));
    expect(maxY).toBeGreaterThan(sag * 0.98);
    expect(maxY).toBeLessThan(sag * 1.02);
  });

  test("endpoints match a and b", () => {
    const a = { x: 12, y: 34 };
    const b = { x: 300, y: 90 };
    const pts = points(cable(a, b, 40));
    expect(pts[0]).toEqual(a);
    expect(pts[pts.length - 1]).toEqual(b);
  });
});

describe("sagFor", () => {
  test("grows with span, clamps, and stays slacker for short cables", () => {
    expect(sagFor({ x: 0, y: 0 }, { x: 10, y: 0 })).toBeCloseTo(36.2);
    expect(sagFor({ x: 0, y: 0 }, { x: 300, y: 0 })).toBeCloseTo(100);
    expect(sagFor({ x: 0, y: 0 }, { x: 2000, y: 0 })).toBe(170);
  });
});

describe("streamColor", () => {
  test("maps id 0 to --color-s0", () => {
    expect(streamColor(0)).toBe("var(--color-s0)");
  });

  test("maps id 7 to --color-s1 (cycles mod 6)", () => {
    expect(streamColor(7)).toBe("var(--color-s1)");
  });

  test("covers all 6 slots without wrapping within range", () => {
    for (let i = 0; i < 6; i++) {
      expect(streamColor(i)).toBe(`var(--color-s${i})`);
    }
  });
});
