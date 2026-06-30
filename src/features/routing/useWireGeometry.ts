export type Pt = { x: number; y: number };

export function curve(a: Pt, b: Pt, centerX: number): string {
  const dx = Math.max(46, Math.abs(b.x - a.x) * 0.42);
  const ad = a.x < centerX ? 1 : -1;
  const bd = b.x < centerX ? 1 : -1;
  return `M${a.x},${a.y} C${a.x + ad * dx},${a.y} ${b.x + bd * dx},${b.y} ${b.x},${b.y}`;
}

export function streamColor(streamId: number): string {
  return `var(--color-s${streamId % 6})`;
}
