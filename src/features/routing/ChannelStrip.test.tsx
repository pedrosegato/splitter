import { render, act, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { StreamSnapshot } from "@/bindings";
import { ChannelStrip } from "./ChannelStrip";
import { ChannelDock } from "./ChannelDock";

const mockStreamControlMutate = vi.fn();
const mockCloseStreamMutate = vi.fn();
const mockSelectStream = vi.fn();

vi.mock("@/hooks/useStreams", () => ({
  useStreamControl: () => ({ mutate: mockStreamControlMutate }),
  useCloseStream: () => ({ mutate: mockCloseStreamMutate }),
}));

vi.mock("@/stores/ui", () => ({
  useUiStore: (selector: (s: { selectStream: typeof mockSelectStream; selectedStreamId: number | null }) => unknown) =>
    selector({ selectStream: mockSelectStream, selectedStreamId: null }),
}));

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
}

function makeStream(overrides: Partial<StreamSnapshot> = {}): StreamSnapshot {
  return {
    id: 1,
    state: "active",
    source_peer: "peer-a",
    sink_peer: "peer-b",
    udp_port: 9001,
    source_device: "MacBook Mic",
    sink_device: "PC Speaker",
    volume: 0.7,
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
});

describe("ChannelStrip", () => {
  it("renders route label as source_device → sink_device", () => {
    const stream = makeStream();
    const { getByText } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={false} />,
      { wrapper: makeWrapper() },
    );

    expect(getByText("MacBook Mic → PC Speaker")).toBeDefined();
  });

  it("clicking M button calls streamControl.mutate with set_muted action", () => {
    const stream = makeStream();
    const { getByLabelText } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={false} />,
      { wrapper: makeWrapper() },
    );

    act(() => {
      fireEvent.click(getByLabelText("mutar"));
    });

    expect(mockStreamControlMutate).toHaveBeenCalledWith({
      sessionId: "sess-1",
      streamId: 1,
      action: "set_muted",
      value: 1,
    });
  });

  it("clicking M again toggles mute off and calls set_muted with value 0", () => {
    const stream = makeStream();
    const { getByLabelText } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={false} />,
      { wrapper: makeWrapper() },
    );

    act(() => {
      fireEvent.click(getByLabelText("mutar"));
    });

    act(() => {
      fireEvent.click(getByLabelText("desmutar"));
    });

    expect(mockStreamControlMutate).toHaveBeenLastCalledWith({
      sessionId: "sess-1",
      streamId: 1,
      action: "set_muted",
      value: 0,
    });
  });

  it("close button calls closeStream.mutate with correct args", () => {
    const stream = makeStream();
    const { getByLabelText } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={false} />,
      { wrapper: makeWrapper() },
    );

    act(() => {
      fireEvent.click(getByLabelText("fechar stream"));
    });

    expect(mockCloseStreamMutate).toHaveBeenCalledWith({
      sessionId: "sess-1",
      streamId: 1,
    });
  });

  it("clicking the strip calls selectStream with stream id", () => {
    const stream = makeStream();
    const { getByRole } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={false} />,
      { wrapper: makeWrapper() },
    );

    act(() => {
      fireEvent.click(getByRole("button", { name: /macbook mic/i }));
    });

    expect(mockSelectStream).toHaveBeenCalledWith(1);
  });

  it("renders slider at correct initial volume (70 for volume 0.7)", () => {
    const stream = makeStream({ volume: 0.7 });
    const { getByRole } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={false} />,
      { wrapper: makeWrapper() },
    );

    const slider = getByRole("slider");
    expect(slider).toBeDefined();
    expect(slider.getAttribute("aria-valuenow")).toBe("70");
  });

  it("applies selected style when selected prop is true", () => {
    const stream = makeStream();
    const { getByRole } = render(
      <ChannelStrip sessionId="sess-1" stream={stream} selected={true} />,
      { wrapper: makeWrapper() },
    );

    const strip = getByRole("button", { name: /macbook mic/i });
    expect(strip.style.boxShadow).toBe("inset 3px 0 0 var(--color-gold)");
  });
});

describe("ChannelDock", () => {
  it("shows 'sem streams' when streams array is empty", () => {
    const { getByText } = render(
      <ChannelDock sessionId="sess-1" streams={[]} />,
      { wrapper: makeWrapper() },
    );

    expect(getByText("sem streams")).toBeDefined();
  });

  it("shows 'sem streams' when sessionId is null", () => {
    const stream = makeStream();
    const { getByText } = render(
      <ChannelDock sessionId={null} streams={[stream]} />,
      { wrapper: makeWrapper() },
    );

    expect(getByText("sem streams")).toBeDefined();
  });

  it("renders one strip per stream", () => {
    const streams = [makeStream({ id: 1 }), makeStream({ id: 2, source_device: "Guitar", sink_device: "Studio Out" })];
    const { getByText } = render(
      <ChannelDock sessionId="sess-1" streams={streams} />,
      { wrapper: makeWrapper() },
    );

    expect(getByText("MacBook Mic → PC Speaker")).toBeDefined();
    expect(getByText("Guitar → Studio Out")).toBeDefined();
  });
});
