import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { useDevices } from "./useDevices";
import { useSetSetting } from "./useSettings";
import { useIdentity } from "./useIdentity";

vi.mock("@/lib/api", () => ({
  commands: {
    identity: vi.fn(),
    listDevices: vi.fn(),
    settingsGet: vi.fn(),
    settingsSet: vi.fn(),
    discoveredPeers: vi.fn(),
    pendingPeers: vi.fn(),
    connectPeer: vi.fn(),
    acceptPending: vi.fn(),
    disconnect: vi.fn(),
    snapshot: vi.fn(),
    openSession: vi.fn(),
    openStream: vi.fn(),
    closeStream: vi.fn(),
    streamControl: vi.fn(),
  },
  unwrap: vi.fn((p: Promise<{ status: "ok"; data: unknown } | { status: "error"; error: string }>) =>
    p.then((r) => {
      if (r.status === "ok") return r.data;
      throw new Error(r.status === "error" ? r.error : "unknown");
    }),
  ),
}));

import { commands, unwrap } from "@/lib/api";

const mockedCommands = commands as unknown as Record<string, ReturnType<typeof vi.fn>>;
const mockedUnwrap = unwrap as unknown as ReturnType<typeof vi.fn>;

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    queryClient,
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    ),
  };
}

describe("useDevices", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("returns the mocked devices list", async () => {
    const devices = [
      { id: "d1", name: "Mic", kind: "Input", default_sample_rate: 44100, channels: 2 },
    ];
    mockedCommands.listDevices.mockResolvedValue({ status: "ok", data: devices });
    mockedUnwrap.mockImplementation((p: Promise<unknown>) =>
      (p as Promise<{ status: string; data: unknown }>).then((r: { status: string; data: unknown }) => r.data),
    );

    const { wrapper } = makeWrapper();
    const { result } = renderHook(() => useDevices(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual(devices);
    expect(mockedCommands.listDevices).toHaveBeenCalledOnce();
  });

  it("exposes error when command fails", async () => {
    mockedCommands.listDevices.mockResolvedValue({ status: "error", error: "no devices" });
    mockedUnwrap.mockImplementation((p: Promise<unknown>) =>
      (p as Promise<{ status: string; error: string }>).then((r: { status: string; error: string }) => {
        throw new Error(r.error);
      }),
    );

    const { wrapper } = makeWrapper();
    const { result } = renderHook(() => useDevices(), { wrapper });

    await waitFor(() => expect(result.current.isError).toBe(true));
    expect((result.current.error as Error).message).toBe("no devices");
  });
});

describe("useSetSetting mutation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("calls settingsSet with mapped args and invalidates settings", async () => {
    const updatedSettings = { auto_accept_trusted: true };
    mockedCommands.settingsSet.mockResolvedValue({ status: "ok", data: updatedSettings });
    mockedUnwrap.mockImplementation((p: Promise<unknown>) =>
      (p as Promise<{ status: string; data: unknown }>).then((r: { status: string; data: unknown }) => r.data),
    );

    const { wrapper, queryClient } = makeWrapper();
    const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");

    const { result } = renderHook(() => useSetSetting(), { wrapper });

    result.current.mutate({ key: "auto_accept_trusted", value: "true" });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(mockedCommands.settingsSet).toHaveBeenCalledWith("auto_accept_trusted", "true");
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["settings"] });
  });
});

describe("useIdentity", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("returns local peer id and name", async () => {
    const identity = { peer_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", peer_name: "Studio PC" };
    mockedCommands.identity.mockResolvedValue({ status: "ok", data: identity });
    mockedUnwrap.mockImplementation((p: Promise<unknown>) =>
      (p as Promise<{ status: string; data: unknown }>).then((r: { status: string; data: unknown }) => r.data),
    );

    const { wrapper } = makeWrapper();
    const { result } = renderHook(() => useIdentity(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual(identity);
    expect(mockedCommands.identity).toHaveBeenCalledOnce();
  });
});
