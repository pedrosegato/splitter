import { render, fireEvent, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { IncomingRequestDialog } from "./IncomingRequestDialog";

const mockAccept = vi.fn();
const mockReject = vi.fn();

vi.mock("@/hooks/usePeers", () => ({
  usePendingPeers: () => ({
    data: [{ peer_id: "pending-1", peer_name: "Notebook João", addr: "192.168.0.44:7373" }],
  }),
}));

vi.mock("@/hooks/useConnection", () => ({
  useAcceptPending: () => ({ mutate: mockAccept, isPending: false }),
  useRejectPending: () => ({ mutate: mockReject, isPending: false }),
}));

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("IncomingRequestDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("auto-shows the incoming peer and accepts on Aceitar", () => {
    render(<IncomingRequestDialog />, { wrapper: makeWrapper() });

    expect(within(document.body).getByText("Notebook João")).toBeTruthy();
    fireEvent.click(within(document.body).getByRole("button", { name: /Aceitar/i }));
    expect(mockAccept).toHaveBeenCalledWith({ index: 0 });
  });

  it("rejects on Recusar", () => {
    render(<IncomingRequestDialog />, { wrapper: makeWrapper() });

    fireEvent.click(within(document.body).getByRole("button", { name: /Recusar/i }));
    expect(mockReject).toHaveBeenCalledWith({ index: 0 });
  });
});
