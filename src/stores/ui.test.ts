import { describe, it, expect, beforeEach } from "vitest";
import { useUiStore } from "./ui";

beforeEach(() => {
  useUiStore.setState({
    activeTab: "routing",
    selectedStreamId: null,
    arm: null,
    stats: [],
    incoming: null,
  });
});

describe("useUiStore", () => {
  it("starts with default state", () => {
    const s = useUiStore.getState();
    expect(s.activeTab).toBe("routing");
    expect(s.selectedStreamId).toBeNull();
    expect(s.arm).toBeNull();
    expect(s.stats).toEqual([]);
    expect(s.incoming).toBeNull();
  });

  it("setTab switches the active tab", () => {
    useUiStore.getState().setTab("stats");
    expect(useUiStore.getState().activeTab).toBe("stats");
  });

  it("setTab back to routing", () => {
    useUiStore.getState().setTab("stats");
    useUiStore.getState().setTab("routing");
    expect(useUiStore.getState().activeTab).toBe("routing");
  });

  it("selectStream sets selectedStreamId", () => {
    useUiStore.getState().selectStream(7);
    expect(useUiStore.getState().selectedStreamId).toBe(7);
  });

  it("selectStream accepts null to clear", () => {
    useUiStore.getState().selectStream(7);
    useUiStore.getState().selectStream(null);
    expect(useUiStore.getState().selectedStreamId).toBeNull();
  });

  it("armSource sets arm and clearArm removes it", () => {
    useUiStore.getState().armSource("peer-1", "device-abc");
    expect(useUiStore.getState().arm).toEqual({ peerId: "peer-1", deviceId: "device-abc" });

    useUiStore.getState().clearArm();
    expect(useUiStore.getState().arm).toBeNull();
  });

  it("setStats replaces stats array", () => {
    const first = [
      { session_id: "s1", stream_id: 1, rtt_ms: 10, loss_pct: 0, kbps_sent: 100, kbps_received: 50 },
    ];
    useUiStore.getState().setStats(first);
    expect(useUiStore.getState().stats).toEqual(first);

    const second = [
      { session_id: "s2", stream_id: 2, rtt_ms: 20, loss_pct: 1, kbps_sent: 200, kbps_received: 100 },
      { session_id: "s3", stream_id: 3, rtt_ms: 30, loss_pct: 2, kbps_sent: 300, kbps_received: 150 },
    ];
    useUiStore.getState().setStats(second);
    expect(useUiStore.getState().stats).toEqual(second);
    expect(useUiStore.getState().stats).toHaveLength(2);
  });

  it("setIncoming stores incoming session", () => {
    useUiStore.getState().setIncoming({ peerId: "p1", peerName: "Alice" });
    expect(useUiStore.getState().incoming).toEqual({ peerId: "p1", peerName: "Alice" });
  });

  it("setIncoming with null clears incoming", () => {
    useUiStore.getState().setIncoming({ peerId: "p1", peerName: "Alice" });
    useUiStore.getState().setIncoming(null);
    expect(useUiStore.getState().incoming).toBeNull();
  });
});
