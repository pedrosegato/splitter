import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { PendingStreamRequestDto } from "@/bindings";

import { StreamRequestDialog } from "./StreamRequestDialog";

const mockAccept = vi.fn();
const mockReject = vi.fn();
const mockState: { requests: PendingStreamRequestDto[] } = { requests: [] };

vi.mock("@/hooks/useStreamRequests", () => ({
  useStreamRequests: () => ({ data: mockState.requests }),
  useAcceptStreamRequest: () => ({ mutate: mockAccept, isPending: false }),
  useRejectStreamRequest: () => ({ mutate: mockReject, isPending: false }),
}));

const micRequest: PendingStreamRequestDto = {
  id: "req-1",
  peer_id: "peer-1",
  peer_name: "Notebook João",
  source_is_system: false,
  source_device_name: "Studio Mic",
};

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

describe("StreamRequestDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.requests = [micRequest];
  });

  it("names the requesting peer and the microphone, and allows on Permitir", () => {
    render(<StreamRequestDialog />, { wrapper: makeWrapper() });

    const body = within(document.body);
    expect(body.getByText("Notebook João")).toBeTruthy();
    expect(body.getByText(/Studio Mic/)).toBeTruthy();
    fireEvent.click(body.getByRole("button", { name: /Permitir/i }));
    expect(mockAccept).toHaveBeenCalledWith({ requestId: "req-1" });
  });

  it("rejects on Recusar", () => {
    render(<StreamRequestDialog />, { wrapper: makeWrapper() });

    fireEvent.click(within(document.body).getByRole("button", { name: /Recusar/i }));
    expect(mockReject).toHaveBeenCalledWith({ requestId: "req-1" });
  });

  it("calls out a system-audio request explicitly", () => {
    mockState.requests = [
      { ...micRequest, source_is_system: true, source_device_name: "Speakers" },
    ];
    render(<StreamRequestDialog />, { wrapper: makeWrapper() });

    expect(within(document.body).getByText(/áudio do sistema/i)).toBeTruthy();
  });

  it("stays closed when nothing is pending", () => {
    mockState.requests = [];
    render(<StreamRequestDialog />, { wrapper: makeWrapper() });

    expect(document.body.querySelector('[data-slot="dialog-content"]')).toBeNull();
  });
});
