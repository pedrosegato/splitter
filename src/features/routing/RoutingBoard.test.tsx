import { render } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { StreamSnapshot, SessionSnapshot, DeviceInfo, IdentityDto, DiscoveredPeer } from "@/bindings";

vi.mock("@/hooks/useIdentity");
vi.mock("@/hooks/useDevices");
vi.mock("@/hooks/useSnapshot");
vi.mock("@/hooks/usePeers");
vi.mock("@/hooks/useConnection", () => ({
  useDisconnect: vi.fn(),
  useConnectPeer: vi.fn(() => ({ mutate: vi.fn(), isPending: false })),
  useAcceptPending: vi.fn(() => ({ mutate: vi.fn(), isPending: false })),
  useOpenSession: vi.fn(() => ({ mutate: vi.fn(), isPending: false })),
}));
vi.mock("@/hooks/useStreams", () => ({
  useStreamControl: vi.fn(() => ({ mutate: vi.fn() })),
  useCloseStream: vi.fn(() => ({ mutate: vi.fn() })),
  useOpenStream: vi.fn(() => ({ mutate: vi.fn() })),
}));
vi.mock("@/features/routing/useWiring");

import { useIdentity } from "@/hooks/useIdentity";
import { useDevices } from "@/hooks/useDevices";
import { useSnapshot } from "@/hooks/useSnapshot";
import { usePeers, usePendingPeers } from "@/hooks/usePeers";
import { useDisconnect } from "@/hooks/useConnection";
import { useWiring } from "@/features/routing/useWiring";

const mockedUseIdentity = useIdentity as ReturnType<typeof vi.fn>;
const mockedUseDevices = useDevices as ReturnType<typeof vi.fn>;
const mockedUseSnapshot = useSnapshot as ReturnType<typeof vi.fn>;
const mockedUsePeers = usePeers as ReturnType<typeof vi.fn>;
const mockedUsePendingPeers = usePendingPeers as ReturnType<typeof vi.fn>;
const mockedUseDisconnect = useDisconnect as ReturnType<typeof vi.fn>;
const mockedUseWiring = useWiring as ReturnType<typeof vi.fn>;

const IDENTITY: IdentityDto = { peer_id: "peer-self", peer_name: "Este Mac" };

const DEVICES: DeviceInfo[] = [
  { id: "mic-1", name: "Microfone USB", kind: "Input", default_sample_rate: 44100, channels: 2 },
  { id: "spk-1", name: "Alto-falantes", kind: "Output", default_sample_rate: 48000, channels: 2 },
  { id: "sys-1", name: "Sistema", kind: "SystemAudio", default_sample_rate: 48000, channels: 2 },
];

const STREAM: StreamSnapshot = {
  id: 1,
  state: "active",
  source_peer: "peer-remote",
  sink_peer: "peer-self",
  udp_port: 9001,
  source_device: "remote-mic",
  sink_device: "spk-1",
  volume: 0.8,
};

const SESSION: SessionSnapshot = {
  id: "sess-1",
  remote_peer_id: "peer-remote",
  state: "active",
  streams: [STREAM],
};

const PEERS: DiscoveredPeer[] = [
  { peer_id: "peer-remote", peer_name: "Studio PC", host: "192.168.1.10", port: 7000, version: "0.1.0" },
];

function makeWrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

beforeEach(() => {
  vi.clearAllMocks();

  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });

  mockedUseIdentity.mockReturnValue({ data: IDENTITY });
  mockedUseDevices.mockReturnValue({ data: DEVICES });
  mockedUseSnapshot.mockReturnValue({ data: [SESSION] });
  mockedUsePeers.mockReturnValue({ data: PEERS });
  mockedUsePendingPeers.mockReturnValue({ data: [] });
  mockedUseDisconnect.mockReturnValue({ mutate: vi.fn() });
  mockedUseWiring.mockReturnValue({ onPortActivate: vi.fn(), hint: null, arm: null });
});

import { RoutingBoard } from "./RoutingBoard";

describe("RoutingBoard — with session", () => {
  it("left panel shows self device names under DESTINOS and FONTES", () => {
    const { getByText, getAllByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });

    const destinosLabels = getAllByText("DESTINOS");
    expect(destinosLabels.length).toBeGreaterThanOrEqual(1);

    const fontesLabels = getAllByText("FONTES");
    expect(fontesLabels.length).toBeGreaterThanOrEqual(1);

    expect(getByText("Alto-falantes")).toBeDefined();
    expect(getByText("Microfone USB")).toBeDefined();
    expect(getByText("Sistema")).toBeDefined();
  });

  it("right panel renders as connected (no 'Conectar máquina' button)", () => {
    const { queryByRole } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(queryByRole("button", { name: /Conectar máquina/i })).toBeNull();
  });

  it("right panel shows remote peer name from discovered peers", () => {
    const { getByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(getByText("Studio PC")).toBeDefined();
  });

  it("ChannelDock renders 1 channel strip for the session stream", () => {
    const { getByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(getByText("remote-mic → spk-1")).toBeDefined();
  });

  it("WireLayer SVG is present in the DOM", () => {
    const { container } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    const svg = container.querySelector("svg");
    expect(svg).toBeDefined();
  });

  it("shows ESTE PC badge on the left panel", () => {
    const { getByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(getByText("ESTE PC")).toBeDefined();
  });
});

describe("RoutingBoard — no session", () => {
  beforeEach(() => {
    mockedUseSnapshot.mockReturnValue({ data: [] });
  });

  it("right panel shows 'Conectar máquina' button", () => {
    const { getByRole } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(getByRole("button", { name: /Conectar máquina/i })).toBeDefined();
  });

  it("ChannelDock shows 'sem streams'", () => {
    const { getByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(getByText("sem streams")).toBeDefined();
  });
});

describe("RoutingBoard — hint toast", () => {
  it("renders the hint toast when useWiring returns a hint", () => {
    mockedUseWiring.mockReturnValue({
      onPortActivate: vi.fn(),
      hint: "clique num destino do outro PC",
      arm: null,
    });

    const { getByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(getByText("clique num destino do outro PC")).toBeDefined();
  });

  it("does not render a hint element when hint is null", () => {
    const { queryByText } = render(<RoutingBoard />, { wrapper: makeWrapper() });
    expect(queryByText("clique num destino do outro PC")).toBeNull();
  });
});
