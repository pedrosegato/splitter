import { render } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { StreamStat, SessionSnapshot } from "@/bindings";
import { aggregate } from "./aggregate";

const mockUseUiStoreSelector = vi.fn();
const mockUseSnapshot = vi.fn();

vi.mock("@/stores/ui", () => ({
  useUiStore: (selector: (s: { stats: StreamStat[] }) => unknown) =>
    mockUseUiStoreSelector(selector),
}));

vi.mock("@/hooks/useSnapshot", () => ({
  useSnapshot: () => mockUseSnapshot(),
}));

import { StatsView } from "./StatsView";

const twoStats: StreamStat[] = [
  {
    session_id: "sess-1",
    stream_id: 1,
    rtt_ms: 10,
    loss_pct: 0.5,
    kbps_sent: 100,
    kbps_received: 50,
  },
  {
    session_id: "sess-1",
    stream_id: 2,
    rtt_ms: 30,
    loss_pct: 1.5,
    kbps_sent: 200,
    kbps_received: 80,
  },
];

const twoStreamSessions: SessionSnapshot[] = [
  {
    id: "sess-1",
    remote_peer_id: "peer-remote",
    state: "active",
    streams: [
      {
        id: 1,
        state: "active",
        source_peer: "peer-local",
        sink_peer: "peer-remote",
        udp_port: 9001,
        source_device: "MacBook Mic",
        sink_device: "Studio Monitors",
        volume: 1,
      },
      {
        id: 2,
        state: "active",
        source_peer: "peer-local",
        sink_peer: "peer-remote",
        udp_port: 9002,
        source_device: "Sistema",
        sink_device: "Fones",
        volume: 0.8,
      },
    ],
  },
];

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

function setupMocks(stats: StreamStat[], sessions: SessionSnapshot[]) {
  mockUseUiStoreSelector.mockImplementation(
    (selector: (s: { stats: StreamStat[] }) => unknown) =>
      selector({ stats }),
  );
  mockUseSnapshot.mockReturnValue({ data: sessions });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
});

describe("aggregate", () => {
  it("returns zeros for empty stats", () => {
    const result = aggregate([]);
    expect(result).toEqual({ avgRtt: 0, avgLoss: 0, totalKbps: 0 });
  });

  it("computes avgRtt as mean of rtt_ms values", () => {
    const result = aggregate(twoStats);
    expect(result.avgRtt).toBe(20);
  });

  it("computes avgLoss as mean of loss_pct values", () => {
    const result = aggregate(twoStats);
    expect(result.avgLoss).toBeCloseTo(1.0, 5);
  });

  it("computes totalKbps as sum of kbps_sent + kbps_received for all entries", () => {
    const result = aggregate(twoStats);
    expect(result.totalKbps).toBe(430);
  });
});

describe("StatsView", () => {
  it("renders aggregate card for streams ativos with count from active session", () => {
    setupMocks(twoStats, twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("streams ativos")).toBeDefined();
    expect(getByText("2")).toBeDefined();
  });

  it("renders latência média card with avg rtt rounded", () => {
    setupMocks(twoStats, twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("latência média")).toBeDefined();
    expect(getByText("20")).toBeDefined();
  });

  it("renders perda média card with one decimal", () => {
    setupMocks(twoStats, twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("perda média")).toBeDefined();
    expect(getByText("1.0")).toBeDefined();
  });

  it("renders banda total card with sum of all kbps", () => {
    setupMocks(twoStats, twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("banda total")).toBeDefined();
    expect(getByText("430")).toBeDefined();
  });

  it("renders sample rate card with static 48 kHz", () => {
    setupMocks(twoStats, twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("sample rate")).toBeDefined();
    expect(getByText("48")).toBeDefined();
  });

  it("renders one row per stat entry with source_device → sink_device label", () => {
    setupMocks(twoStats, twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("MacBook Mic → Studio Monitors")).toBeDefined();
    expect(getByText("Sistema → Fones")).toBeDefined();
  });

  it("shows fallback label when stream snapshot is not found", () => {
    setupMocks(twoStats, []);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("stream 1")).toBeDefined();
    expect(getByText("stream 2")).toBeDefined();
  });

  it("shows sem streams ativos when stats array is empty", () => {
    setupMocks([], twoStreamSessions);
    const { getByText } = render(<StatsView />, { wrapper: makeWrapper() });
    expect(getByText("sem streams ativos")).toBeDefined();
  });
});
