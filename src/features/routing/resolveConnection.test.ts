import { describe, it, expect } from "vitest";
import { resolveConnection } from "./resolveConnection";

const src = (peerId: string, deviceId: string) => ({ peerId, deviceId, kind: "src" as const });
const sink = (peerId: string, deviceId: string) => ({ peerId, deviceId, kind: "sink" as const });

describe("resolveConnection", () => {
  it("pairs opposite kinds across different peers", () => {
    expect(resolveConnection(src("A", "mic"), sink("B", "spk"))).toEqual({
      src: { peer: "A", dev: "mic" },
      sink: { peer: "B", dev: "spk" },
    });
  });

  it("normalizes order (sink first, src second)", () => {
    expect(resolveConnection(sink("B", "spk"), src("A", "mic"))).toEqual({
      src: { peer: "A", dev: "mic" },
      sink: { peer: "B", dev: "spk" },
    });
  });

  it("rejects same kind", () => {
    expect(resolveConnection(src("A", "mic"), src("B", "mic2"))).toBeNull();
  });

  it("rejects same peer", () => {
    expect(resolveConnection(src("A", "mic"), sink("A", "spk"))).toBeNull();
  });
});
