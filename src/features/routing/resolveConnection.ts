export type PortRef = { peerId: string; deviceId: string; kind: "src" | "sink" };
export type Connection = {
  src: { peer: string; dev: string };
  sink: { peer: string; dev: string };
} | null;

export function resolveConnection(a: PortRef, b: PortRef): Connection {
  if (a.kind === b.kind) return null;
  if (a.peerId === b.peerId) return null;
  const source = a.kind === "src" ? a : b;
  const target = a.kind === "sink" ? a : b;
  return {
    src: { peer: source.peerId, dev: source.deviceId },
    sink: { peer: target.peerId, dev: target.deviceId },
  };
}
