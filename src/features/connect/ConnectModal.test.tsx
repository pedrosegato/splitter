import { render, fireEvent, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { ConnectModal } from "./ConnectModal";

const mockConnectMutate = vi.fn();
const mockOpenSessionMutate = vi.fn();

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
}));

vi.mock("@/hooks/useConnection", () => ({
  useConnectPeer: () => ({ mutate: mockConnectMutate, isPending: false }),
  useOpenSession: () => ({ mutate: mockOpenSessionMutate, isPending: false }),
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

  it("renders the discovered peer as a clickable row", () => {
    render(<ConnectModal open onOpenChange={vi.fn()} />, { wrapper: makeWrapper() });

    expect(document.body.querySelector('[data-slot="dialog-content"]')).toBeTruthy();
    expect(within(document.body).getByRole("button", { name: /Studio PC/i })).toBeTruthy();
  });

  it("pairs when the whole row is clicked", () => {
    render(<ConnectModal open onOpenChange={vi.fn()} />, { wrapper: makeWrapper() });

    fireEvent.click(within(document.body).getByRole("button", { name: /Studio PC/i }));

    expect(mockConnectMutate).toHaveBeenCalledWith(
      { host: "192.168.0.21", port: 7373, peerId: "peer-1" },
      expect.objectContaining({ onSuccess: expect.any(Function) }),
    );
  });

  it("calls onOpenChange(false) when cancelar is clicked", () => {
    const onOpenChange = vi.fn();
    render(<ConnectModal open onOpenChange={onOpenChange} />, { wrapper: makeWrapper() });

    fireEvent.click(within(document.body).getByRole("button", { name: "cancelar" }));

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
