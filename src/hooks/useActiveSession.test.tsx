import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { type ReactNode } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { useActiveSession } from "./useActiveSession";

vi.mock("@/lib/api", () => ({
  commands: {
    snapshot: vi.fn(),
    peerDevices: vi.fn(),
  },
  unwrap: vi.fn((p: Promise<{ status: "ok"; data: unknown } | { status: "error"; error: string }>) =>
    p.then((r) => {
      if (r.status === "ok") return r.data;
      throw new Error(r.status === "error" ? r.error : "unknown");
    }),
  ),
}));

import { commands } from "@/lib/api";

const mockedCommands = commands as unknown as Record<string, ReturnType<typeof vi.fn>>;

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

function makeSession(overrides: Record<string, unknown> = {}) {
  return {
    id: "sess-1",
    remote_peer_id: "peer-remote",
    state: "active",
    streams: [{ id: 1 }],
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("useActiveSession", () => {
  it("picks the active session and exposes its streams and remote peer id", async () => {
    const active = makeSession();
    mockedCommands.snapshot.mockResolvedValue({
      status: "ok",
      data: [makeSession({ id: "sess-old", state: "closed", streams: [] }), active],
    });
    mockedCommands.peerDevices.mockResolvedValue({ status: "ok", data: [] });

    const { result } = renderHook(() => useActiveSession(), { wrapper: makeWrapper() });

    await waitFor(() => expect(result.current.session?.state).toBe("active"));
    expect(result.current.remotePeerId).toBe("peer-remote");
    expect(result.current.streams).toEqual(active.streams);
  });

  it("returns nulls when there is no active session", async () => {
    mockedCommands.snapshot.mockResolvedValue({
      status: "ok",
      data: [makeSession({ state: "closed" })],
    });

    const { result } = renderHook(() => useActiveSession(), { wrapper: makeWrapper() });

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.session).toBeNull();
    expect(result.current.streams).toEqual([]);
    expect(result.current.remotePeerId).toBeUndefined();
  });

  it("does not fetch peer devices when there is no active session", async () => {
    mockedCommands.snapshot.mockResolvedValue({ status: "ok", data: [] });

    const { result } = renderHook(() => useActiveSession(), { wrapper: makeWrapper() });

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(mockedCommands.peerDevices).not.toHaveBeenCalled();
  });
});
