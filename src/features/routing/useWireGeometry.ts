export type Pt = { x: number; y: number };

const K = 1.7;
const COSH_K = Math.cosh(K);

export function catenaryDroop(t: number): number {
  const u = 2 * t - 1;
  return (COSH_K - Math.cosh(K * u)) / (COSH_K - 1);
}

export function sagFor(a: Pt, b: Pt): number {
  const span = Math.hypot(b.x - a.x, b.y - a.y);
  return Math.min(170, 34 + span * 0.22);
}

const r = (n: number) => Math.round(n * 10) / 10;

export function cable(a: Pt, b: Pt, sag: number, samples = 26): string {
  let d = `M${r(a.x)},${r(a.y)}`;
  for (let i = 1; i <= samples; i++) {
    const t = i / samples;
    const x = a.x + (b.x - a.x) * t;
    const y = a.y + (b.y - a.y) * t + sag * catenaryDroop(t);
    d += ` L${r(x)},${r(y)}`;
  }
  return d;
}

export function streamColor(streamId: number): string {
  return `var(--color-s${streamId % 6})`;
}
