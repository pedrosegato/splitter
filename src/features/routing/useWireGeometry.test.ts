import { describe, expect, test } from "vitest";
import { curve, streamColor } from "./useWireGeometry";

describe("curve", () => {
  test("emits a cubic bezier starting at point a", () => {
    const d = curve({ x: 260, y: 100 }, { x: 560, y: 200 }, 410);
    expect(d.startsWith("M260,100 C")).toBe(true);
  });

  test("control points exit outward by panel side", () => {
    const centerX = 410;
    const a = { x: 260, y: 100 };
    const b = { x: 560, y: 200 };

    const d = curve(a, b, centerX);

    const match = d.match(
      /^M(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?) C(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?) (-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?) (-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)$/,
    );
    expect(match).not.toBeNull();

    const cp1x = Number(match![3]);
    const cp2x = Number(match![5]);

    expect(cp1x).toBeGreaterThan(a.x);
    expect(cp2x).toBeLessThan(b.x);
  });

  test("control point dx is at least 46 px", () => {
    const d = curve({ x: 300, y: 50 }, { x: 320, y: 50 }, 410);

    const match = d.match(
      /^M(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?) C(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?) (-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?) (-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)$/,
    );
    expect(match).not.toBeNull();

    const cp1x = Number(match![3]);
    expect(Math.abs(cp1x - 300)).toBeGreaterThanOrEqual(46);
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
