import { renderHook, act, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { useUiStore } from "@/stores/ui";

vi.mock("@/hooks/useSnapshot");
vi.mock("@/hooks/useDevices");
vi.mock("@/hooks/useStreams");

import { useSnapshot } from "@/hooks/useSnapshot";
import { useDevices } from "@/hooks/useDevices";
import { useOpenStream } from "@/hooks/useStreams";

const mockedUseSnapshot = useSnapshot as ReturnType<typeof vi.fn>;
const mockedUseDevices = useDevices as ReturnType<typeof vi.fn>;
const mockedUseOpenStream = useOpenStream as ReturnType<typeof vi.fn>;

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
      incoming: null,
    });

    mutateSpy = vi.fn();
    mockedUseSnapshot.mockReturnValue({ data: [SESSION] });
    mockedUseDevices.mockReturnValue({ data: DEVICES });
    mockedUseOpenStream.mockReturnValue({ mutate: mutateSpy });
  });

  it("sink-first yields hint 'comece por uma fonte'", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:sink:mic-1", "sink", "peer-a", "mic-1");
    });

    expect(result.current.hint).toBe("comece por uma fonte deste PC");
    expect(mutateSpy).not.toHaveBeenCalled();
  });

  it("arm a system src then a sink on another peer calls openStream with sourceIsSystem=true", async () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.arm).toEqual({ peerId: "peer-a", deviceId: "sys-1" });
    expect(result.current.hint).toBe("clique num destino do outro PC");

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
    expect(result.current.hint).toBeNull();
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

  it("src then sink on the SAME peer does not call mutate and clears arm", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.arm).not.toBeNull();

    act(() => {
      result.current.onPortActivate("peer-a:sink:spk-1", "sink", "peer-a", "spk-1");
    });

    expect(mutateSpy).not.toHaveBeenCalled();
    expect(result.current.arm).toBeNull();
    expect(result.current.hint).toBe("não pode rotear pra mesma máquina");
  });

  it("no sessions yields hint 'conecte uma máquina primeiro'", () => {
    mockedUseSnapshot.mockReturnValue({ data: [] });
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortActivate("peer-a:src:sys-1", "src", "peer-a", "sys-1");
    });

    expect(result.current.hint).toBe("conecte uma máquina primeiro");
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
    expect(result.current.hint).toBeNull();
  });

  it("hint auto-clears after 2200ms", () => {
    vi.useFakeTimers();
    try {
      const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

      act(() => {
        result.current.onPortActivate("peer-a:sink:spk-1", "sink", "peer-a", "spk-1");
      });

      expect(result.current.hint).toBe("comece por uma fonte deste PC");

      act(() => {
        vi.advanceTimersByTime(2200);
      });

      expect(result.current.hint).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});
