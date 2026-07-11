import { renderHook, act, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";

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
import type { PortRef } from "./resolveConnection";

describe("useWiring", () => {
  let mutateSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    mutateSpy = vi.fn();
    mockedUseSnapshot.mockReturnValue({ data: [SESSION] });
    mockedUseDevices.mockReturnValue({ data: DEVICES });
    mockedUsePeerDevices.mockReturnValue({ data: [] });
    mockedUseOpenStream.mockReturnValue({ mutate: mutateSpy });
    mockedUseRequestStream.mockReturnValue({ mutate: vi.fn() });
    mockedUseIdentity.mockReturnValue({ data: { peer_id: "peer-a", peer_name: "A" } });
  });

  const src = (peerId: string, deviceId: string): PortRef => ({ peerId, deviceId, kind: "src" });
  const sink = (peerId: string, deviceId: string): PortRef => ({ peerId, deviceId, kind: "sink" });

  it("dragging a self system src to a remote sink calls openStream with sourceIsSystem=true", async () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-a", "sys-1"), sink("peer-b", "spk-1"));
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
  });

  it("dragging a self mic src to a remote sink calls openStream with sourceIsSystem=false", async () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-a", "mic-1"), sink("peer-b", "spk-1"));
    });

    await waitFor(() => expect(mutateSpy).toHaveBeenCalledOnce());
    expect(mutateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ sourceIsSystem: false }),
    );
  });

  it("order-agnostic: dragging sink first then src still resolves the connection", async () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(sink("peer-b", "spk-1"), src("peer-a", "sys-1"));
    });

    await waitFor(() => expect(mutateSpy).toHaveBeenCalledOnce());
    expect(mutateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ sourceDeviceId: "sys-1", sinkDeviceId: "spk-1" }),
    );
  });

  it("dragging a remote system src to a local sink calls requestStream with system SourceKind", async () => {
    const requestMutate = vi.fn();
    mockedUseRequestStream.mockReturnValue({ mutate: requestMutate });
    mockedUsePeerDevices.mockReturnValue({
      data: [{ id: "rsys-1", name: "Remote System", kind: "SystemAudio" }],
    });
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-b", "rsys-1"), sink("peer-a", "spk-1"));
    });

    await waitFor(() => expect(requestMutate).toHaveBeenCalledOnce());
    expect(requestMutate).toHaveBeenCalledWith({
      sessionId: "sess-1",
      source: { type: "system", device_id: "rsys-1" },
      sinkDeviceId: "spk-1",
    });
  });

  it("dragging a remote mic src to a local sink calls requestStream with mic SourceKind", async () => {
    const requestMutate = vi.fn();
    mockedUseRequestStream.mockReturnValue({ mutate: requestMutate });
    mockedUsePeerDevices.mockReturnValue({
      data: [{ id: "rmic-1", name: "Remote Mic", kind: "Input" }],
    });
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-b", "rmic-1"), sink("peer-a", "spk-1"));
    });

    await waitFor(() => expect(requestMutate).toHaveBeenCalledOnce());
    expect(requestMutate).toHaveBeenCalledWith({
      sessionId: "sess-1",
      source: { type: "mic", device_id: "rmic-1" },
      sinkDeviceId: "spk-1",
    });
  });

  it("src to sink on the SAME peer does not connect", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-a", "sys-1"), sink("peer-a", "spk-1"));
    });

    expect(mutateSpy).not.toHaveBeenCalled();
  });

  it("two src ports do not connect", () => {
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-a", "sys-1"), src("peer-b", "mic-9"));
    });

    expect(mutateSpy).not.toHaveBeenCalled();
  });

  it("no session: dragging a connection does nothing", () => {
    mockedUseSnapshot.mockReturnValue({ data: [] });
    const { result } = renderHook(() => useWiring(), { wrapper: makeWrapper() });

    act(() => {
      result.current.onPortConnect(src("peer-a", "sys-1"), sink("peer-b", "spk-1"));
    });

    expect(mutateSpy).not.toHaveBeenCalled();
  });
});
