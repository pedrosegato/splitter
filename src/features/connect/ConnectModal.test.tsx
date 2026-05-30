import { render, fireEvent, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { ConnectModal } from "./ConnectModal";

const mockConnectMutate = vi.fn();
const mockOpenSessionMutate = vi.fn();
const mockAcceptPendingMutate = vi.fn();

vi.mock("@/hooks/usePeers", () => ({
  usePeers: () => ({
    data: [
      {
        peer_id: "peer-1",
        peer_name: "Studio PC",
        host: "192.168.0.21",
        port: 7373,
        version: "0.4.0",
      },
    ],
  }),
  usePendingPeers: () => ({
    data: [
      {
        peer_id: "pending-1",
        peer_name: "Notebook João",
        addr: "192.168.0.44:7373",
      },
    ],
  }),
}));

vi.mock("@/hooks/useConnection", () => ({
  useConnectPeer: () => ({
    mutate: mockConnectMutate,
    isPending: false,
  }),
  useOpenSession: () => ({
    mutate: mockOpenSessionMutate,
    isPending: false,
  }),
  useAcceptPending: () => ({
    mutate: mockAcceptPendingMutate,
    isPending: false,
  }),
}));

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("ConnectModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders peer name and parear button for discovered peer", () => {
    const onOpenChange = vi.fn();
    render(<ConnectModal open onOpenChange={onOpenChange} />, {
      wrapper: makeWrapper(),
    });

    expect(document.body.querySelector('[data-slot="dialog-content"]')).toBeTruthy();
    const content = document.body;
    expect(within(content).getByText("Studio PC")).toBeTruthy();
    expect(within(content).getByRole("button", { name: "parear" })).toBeTruthy();
  });

  it("calls connectPeer.mutate with host, port and peerId when parear is clicked", () => {
    render(<ConnectModal open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const btn = within(document.body).getByRole("button", { name: "parear" });
    fireEvent.click(btn);

    expect(mockConnectMutate).toHaveBeenCalledWith(
      { host: "192.168.0.21", port: 7373, peerId: "peer-1" },
      expect.objectContaining({ onSuccess: expect.any(Function) }),
    );
  });

  it("renders pending peer row and calls acceptPending.mutate with index 0 when aceitar is clicked", () => {
    render(<ConnectModal open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    const content = document.body;
    expect(within(content).getByText("Notebook João")).toBeTruthy();

    const btn = within(content).getByRole("button", { name: "aceitar" });
    fireEvent.click(btn);

    expect(mockAcceptPendingMutate).toHaveBeenCalledWith({ index: 0 });
  });

  it("renders TOFU note", () => {
    render(<ConnectModal open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });

    expect(
      within(document.body).getByText("1ª conexão pede confiar no dispositivo"),
    ).toBeTruthy();
  });

  it("calls onOpenChange(false) when cancelar is clicked", () => {
    const onOpenChange = vi.fn();
    render(<ConnectModal open onOpenChange={onOpenChange} />, {
      wrapper: makeWrapper(),
    });

    const btn = within(document.body).getByRole("button", { name: "cancelar" });
    fireEvent.click(btn);

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows empty state when no discovered peers", () => {
    vi.doMock("@/hooks/usePeers", () => ({
      usePeers: () => ({ data: [] }),
      usePendingPeers: () => ({ data: [] }),
    }));

    render(<ConnectModal open onOpenChange={vi.fn()} />, {
      wrapper: makeWrapper(),
    });
  });
});
