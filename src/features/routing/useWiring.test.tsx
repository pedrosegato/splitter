import { renderHook, act, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { useUiStore } from "@/stores/ui";

vi.mock("@/hooks/useSnapshot");
vi.mock("@/hooks/useDevices");
vi.mock("@/hooks/useStreams");
vi.mock("@/hooks/useIdentity");

import { useSnapshot } from "@/hooks/useSnapshot";
import { useDevices, usePeerDevices } from "@/hooks/useDevices";
import { useOpenStream, useRequestStream } from "@/hooks/useStreams";
import { useIdentity } from "@/hooks/useIdentity";

const mockedUseSnapshot = useSnapshot as ReturnType<typeof vi.fn>;
const mockedUseDevices = useDevices as ReturnType<typeof vi.fn>;
const mockedUsePeerDevices = usePeerDevices as ReturnType<typeof vi.fn>;
const mockedUseOpenStream = useOpenStream as ReturnType<typeof vi.fn>;
const mockedUseRequestStream = useRequestStream as ReturnType<typeof vi.fn>;
const mockedUseIdentity = useIdentity as ReturnType<typeof vi.fn>;

const SESSION = { id: "sess-1", remote_peer_id: "peer-b", state: "active", streams: [] };
const DEVICES = [
  { id: "mic-1", name: "Mic", kind: "Input", default_sample_rate: 44100, channels: 2 },
  { id: "sys-1", name: "SystemAudio", kind: "SystemAudio", default_sample_rate: 48000, channels: 2 },
];

function makeWrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

import { useWiring } from "./useWiring";

describe("useWiring", () => {
  let mutateSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    useUiStore.setState({
      activeTab: "routing",
      selectedStreamId: null,
      arm: null,
      stats: [],
    });

    mutateSpy = vi.fn();
    mockedUseSnapshot.mockReturnValue({ data: [SESSION] });
    mockedUseDevices.mockReturnValue({ data: DEVICES });
    mockedUsePeerDevices.mockReturnValue({ data: [] });
    mockedUseOpenStream.mockReturnValue({ mutate: mutateSpy });
    mockedUseRequestStream.mockReturnValue({ mutate: vi.fn() });
    mockedUseIdentity.mockReturnValue({ data: { peer_id: "peer-a", peer_name: "A" } });
  });

  it("clicking a sink first arms it (order-agnostic, no error)", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:sink:mic-1", "sink", "peer-a", "mic-1");
    });

    expect(result.current.arm).toEqual({
      peerId: "peer-a",
      deviceId: "mic-1",
      kind: "sink",
    });
    expect(mutateSpy).not.toHaveBeenCalled();
  });

  it("arm a system src then a sink on another peer calls openStream with sourceIsSystem=true", async () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.arm).toEqual({
      peerId: "peer-a",
      deviceId: "sys-1",
      kind: "src",
    });

    act(() => {
      result.current.onPortActivate("peer-b:sink:spk-1", "sink", "peer-b", "spk-1");
    });

    await waitFor(() => expect(mutateSpy).toHaveBeenCalledOnce());

    expect(mutateSpy).toHaveBeenCalledWith({
      sessionId: "sess-1",
      sourceDeviceId: "sys-1",
      sourceIsSystem: true,
      sinkPeerId: "peer-b",
      sinkDeviceId: "spk-1",
      bitrate: undefined,
    });
    expect(result.current.arm).toBeNull();
  });

  it("arm a mic src then a sink on another peer calls openStream with sourceIsSystem=false", async () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:mic-1", "src", "peer-a", "mic-1");
    });

    act(() => {
      result.current.onPortActivate("peer-b:sink:spk-1", "sink", "peer-b", "spk-1");
    });

    await waitFor(() => expect(mutateSpy).toHaveBeenCalledOnce());

    expect(mutateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ sourceIsSystem: false }),
    );
  });

  it("src then sink on the SAME peer does not connect", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.arm).not.toBeNull();

    act(() => {
      result.current.onPortActivate("peer-a:sink:spk-1", "sink", "peer-a", "spk-1");
    });

    expect(mutateSpy).not.toHaveBeenCalled();
  });

  it("no session: clicking a port does nothing (no hint, no arm)", () => {
    mockedUseSnapshot.mockReturnValue({ data: [] });
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.arm).toBeNull();
    expect(mutateSpy).not.toHaveBeenCalled();
  });

  it("Escape key clears arm", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.arm).not.toBeNull();

    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });

    expect(result.current.arm).toBeNull();
  });

});
