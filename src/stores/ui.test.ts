import { describe, it, expect, beforeEach } from "vitest";
import { useUiStore, pushStatsHistory, type StreamHistory } from "./ui";
import type { StreamStat } from "@/bindings";

beforeEach(() => {
  useUiStore.setState({
    activeTab: "routing",
    selectedStreamId: null,
    arm: null,
    stats: [],
    statsHistory: {},
  });
});

describe("useUiStore", () => {
  it("starts with default state", () => {
    const s = useUiStore.getState();
    expect(s.activeTab).toBe("routing");
    expect(s.selectedStreamId).toBeNull();
    expect(s.arm).toBeNull();
    expect(s.stats).toEqual([]);
    expect(s.statsHistory).toEqual({});
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
    useUiStore.getState().armSource("peer-1", "device-abc", "src");
    expect(useUiStore.getState().arm).toEqual({
      peerId: "peer-1",
      deviceId: "device-abc",
      kind: "src",
    });

    useUiStore.getState().clearArm();
    expect(useUiStore.getState().arm).toBeNull();
  });

  it("pushStats updates stats and statsHistory", () => {
    const tick: StreamStat[] = [
      { session_id: "s1", stream_id: 1, rtt_ms: 10, loss_pct: 0.5, kbps_sent: 100, kbps_received: 50 },
    ];
    useUiStore.getState().pushStats(tick);
    expect(useUiStore.getState().stats).toEqual(tick);
    expect(useUiStore.getState().statsHistory[1].rtt).toEqual([10]);
    expect(useUiStore.getState().statsHistory[1].loss).toEqual([0.5]);
    expect(useUiStore.getState().statsHistory[1].kbps).toEqual([150]);
  });
});

describe("pushStatsHistory", () => {
  const makeStat = (stream_id: number, rtt_ms: number, loss_pct: number, kbps_sent: number, kbps_received: number): StreamStat => ({
    session_id: "s1",
    stream_id,
    rtt_ms,
    loss_pct,
    kbps_sent,
    kbps_received,
  });

  it("appends values to a new stream entry", () => {
    const result = pushStatsHistory({}, [makeStat(1, 20, 1.5, 100, 50)]);
    expect(result[1].rtt).toEqual([20]);
    expect(result[1].loss).toEqual([1.5]);
    expect(result[1].kbps).toEqual([150]);
  });

  it("appends to existing history on successive calls", () => {
    const after1 = pushStatsHistory({}, [makeStat(1, 10, 0.5, 50, 25)]);
    const after2 = pushStatsHistory(after1, [makeStat(1, 20, 1.0, 100, 50)]);
    expect(after2[1].rtt).toEqual([10, 20]);
    expect(after2[1].loss).toEqual([0.5, 1.0]);
    expect(after2[1].kbps).toEqual([75, 150]);
  });

  it("caps arrays at 60 entries (drops oldest)", () => {
    let history: Record<number, StreamHistory> = {};
    for (let i = 0; i < 65; i++) {
      history = pushStatsHistory(history, [makeStat(1, i, 0, i, 0)]);
    }
    expect(history[1].rtt).toHaveLength(60);
    expect(history[1].rtt[0]).toBe(5);
    expect(history[1].rtt[59]).toBe(64);
  });

  it("handles a new stream id not previously seen", () => {
    const base = pushStatsHistory({}, [makeStat(1, 10, 0, 100, 0)]);
    const result = pushStatsHistory(base, [makeStat(2, 30, 2.0, 200, 100)]);
    expect(result[1].rtt).toEqual([10]);
    expect(result[2].rtt).toEqual([30]);
  });

  it("preserves history of streams absent from the current tick", () => {
    const base = pushStatsHistory({}, [makeStat(1, 10, 0, 100, 0)]);
    const result = pushStatsHistory(base, [makeStat(2, 30, 0, 200, 0)]);
    expect(result[1].rtt).toEqual([10]);
    expect(result[2].rtt).toEqual([30]);
  });

  it("does not mutate the previous history object", () => {
    const prev = {};
    const result = pushStatsHistory(prev, [makeStat(1, 10, 0, 100, 0)]);
    expect(prev).toEqual({});
    expect(result).not.toBe(prev);
  });
});
