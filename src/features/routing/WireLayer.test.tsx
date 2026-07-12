import { render, act, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { useRef } from "react";
import { motionValue } from "motion/react";
import { WireLayer } from "./WireLayer";
import { PortRegistryProvider, usePortRegistry } from "./usePortRegistry";
import type { StreamSnapshot } from "@/bindings";

function makeBoardEl(width = 800): HTMLDivElement {
  const el = document.createElement("div");
  vi.spyOn(el, "getBoundingClientRect").mockReturnValue({
    left: 0, top: 0, right: width, bottom: 600,
    width, height: 600, x: 0, y: 0,
    toJSON: () => ({}),
  } as DOMRect);
  Object.defineProperty(el, "clientWidth", { get: () => width });
  return el;
}

function makePortEl(x: number, y: number): HTMLElement {
  const el = document.createElement("button");
  vi.spyOn(el, "getBoundingClientRect").mockReturnValue({
    left: x - 6, top: y - 6, right: x + 6, bottom: y + 6,
    width: 12, height: 12, x: x - 6, y: y - 6,
    toJSON: () => ({}),
  } as DOMRect);
  return el;
}

function makeStream(
  id: number,
  sourcePeer = "peer-a",
  sinkPeer = "peer-b",
  sourceDevice = "dev-1",
  sinkDevice = "dev-2",
  state: StreamSnapshot["state"] = "active",
): StreamSnapshot {
  return {
    id,
    state,
    source_peer: sourcePeer,
    sink_peer: sinkPeer,
    udp_port: 9000 + id,
    source_device: sourceDevice,
    sink_device: sinkDevice,
    volume: 80,
    muted: false,
  };
}

function RegistrySeeder({
  entries,
}: {
  entries: Array<{ id: string; el: HTMLElement }>;
}) {
  const registry = usePortRegistry();
  for (const { id, el } of entries) {
    registry.register(id, el);
  }
  return null;
}

function Wrapper({
  streams,
  selectedId,
  onSelect,
  registryEntries,
}: {
  streams: StreamSnapshot[];
  selectedId: number | null;
  onSelect: (id: number | null) => void;
  registryEntries: Array<{ id: string; el: HTMLElement }>;
}) {
  const boardRef = useRef<HTMLDivElement | null>(makeBoardEl());
  return (
    <PortRegistryProvider>
      <RegistrySeeder entries={registryEntries} />
      <WireLayer
        boardRef={boardRef}
        streams={streams}
        selectedId={selectedId}
        onSelect={onSelect}
      />
    </PortRegistryProvider>
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
});

describe("WireLayer", () => {
  it("renders one visible wire path per stream", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const stream = makeStream(1);
    const { container } = render(
      <Wrapper
        streams={[stream]}
        selectedId={null}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const paths = container.querySelectorAll("path");
    const visibleWires = Array.from(paths).filter(
      (p) =>
        p.getAttribute("stroke") !== "transparent" &&
        p.getAttribute("stroke-width") === "2.8",
    );
    expect(visibleWires).toHaveLength(1);
  });

  it("renders two visible wire paths for two streams", () => {
    const srcEl1 = makePortEl(100, 150);
    const sinkEl1 = makePortEl(700, 150);
    const srcEl2 = makePortEl(100, 250);
    const sinkEl2 = makePortEl(700, 250);

    const streams = [
      makeStream(0, "peer-a", "peer-b", "dev-1", "dev-2"),
      makeStream(1, "peer-a", "peer-b", "dev-3", "dev-4"),
    ];

    const { container } = render(
      <Wrapper
        streams={streams}
        selectedId={null}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl1 },
          { id: "peer-b:sink:dev-2", el: sinkEl1 },
          { id: "peer-a:src:dev-3", el: srcEl2 },
          { id: "peer-b:sink:dev-4", el: sinkEl2 },
        ]}
      />,
    );

    const paths = container.querySelectorAll("path");
    const visibleWires = Array.from(paths).filter(
      (p) =>
        p.getAttribute("stroke") !== "transparent" &&
        (p.getAttribute("stroke-width") === "2.8" ||
          p.getAttribute("stroke-width") === "4"),
    );
    expect(visibleWires).toHaveLength(2);
  });

  it("every path has fill:none", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const { container } = render(
      <Wrapper
        streams={[makeStream(2)]}
        selectedId={null}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const paths = container.querySelectorAll("path");
    expect(paths.length).toBeGreaterThan(0);
    for (const p of paths) {
      const fill = p.getAttribute("fill");
      expect(fill).toBe("none");
    }
  });

  it("uses the correct stroke color derived from stream id", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const stream = makeStream(0);
    const { container } = render(
      <Wrapper
        streams={[stream]}
        selectedId={null}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const visibleWire = Array.from(container.querySelectorAll("path")).find(
      (p) =>
        p.getAttribute("stroke") !== "transparent" &&
        p.getAttribute("stroke-width") === "2.8",
    );

    expect(visibleWire).toBeDefined();
    expect(visibleWire!.getAttribute("stroke")).toBe("var(--color-s0)");
  });

  it("clicking the hit path calls onSelect with the stream id", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const onSelect = vi.fn();
    const stream = makeStream(5);

    const { container } = render(
      <Wrapper
        streams={[stream]}
        selectedId={null}
        onSelect={onSelect}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const hitPath = Array.from(container.querySelectorAll("path")).find(
      (p) => p.getAttribute("stroke") === "transparent",
    );
    expect(hitPath).toBeDefined();

    act(() => {
      fireEvent.click(hitPath!);
    });

    expect(onSelect).toHaveBeenCalledWith(5);
  });

  it("skips streams where port elements are missing", () => {
    const srcEl = makePortEl(100, 200);

    const stream = makeStream(3);
    const { container } = render(
      <Wrapper
        streams={[stream]}
        selectedId={null}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
        ]}
      />,
    );

    const paths = container.querySelectorAll("path");
    expect(paths.length).toBe(0);
  });

  it("renders flow path for selected non-muted stream", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const { container } = render(
      <Wrapper
        streams={[makeStream(1)]}
        selectedId={1}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const flowPath = Array.from(container.querySelectorAll("path")).find(
      (p) =>
        p.getAttribute("stroke") === "#fff" &&
        p.getAttribute("stroke-dasharray") === "1 10",
    );
    expect(flowPath).toBeDefined();
  });

  it("does not render flow path for muted stream", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const { container } = render(
      <Wrapper
        streams={[makeStream(1, "peer-a", "peer-b", "dev-1", "dev-2", "paused")]}
        selectedId={1}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const flowPath = Array.from(container.querySelectorAll("path")).find(
      (p) =>
        p.getAttribute("stroke") === "#fff" &&
        p.getAttribute("stroke-dasharray") === "1 10",
    );
    expect(flowPath).toBeUndefined();
  });

  it("selected wire uses stroke-width 4", () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const { container } = render(
      <Wrapper
        streams={[makeStream(2)]}
        selectedId={2}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    const selected = Array.from(container.querySelectorAll("path")).find(
      (p) =>
        p.getAttribute("stroke") !== "transparent" &&
        p.getAttribute("stroke") !== "#fff" &&
        p.getAttribute("stroke-width") === "4",
    );
    expect(selected).toBeDefined();
  });

  it("renders no live-drag path when no drag prop is passed", async () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const { container } = render(
      <Wrapper
        streams={[makeStream(1)]}
        selectedId={null}
        onSelect={vi.fn()}
        registryEntries={[
          { id: "peer-a:src:dev-1", el: srcEl },
          { id: "peer-b:sink:dev-2", el: sinkEl },
        ]}
      />,
    );

    await waitFor(() => {
      const goldPaths = Array.from(container.querySelectorAll("path")).filter(
        (p) => p.getAttribute("stroke") === "var(--color-gold)",
      );
      expect(goldPaths).toHaveLength(0);
    });

    const visibleWires = Array.from(container.querySelectorAll("path")).filter(
      (p) =>
        p.getAttribute("stroke") !== "transparent" &&
        p.getAttribute("stroke-width") === "2.8",
    );
    expect(visibleWires).toHaveLength(1);
  });

  it("renders no live-drag path when drag is inactive", async () => {
    const srcEl = makePortEl(100, 200);
    const sinkEl = makePortEl(700, 200);

    const inactiveDrag = {
      active: false,
      from: null,
      x: motionValue(0),
      y: motionValue(0),
    };

    const { container } = render(
      <PortRegistryProvider>
        <RegistrySeeder
          entries={[
            { id: "peer-a:src:dev-1", el: srcEl },
            { id: "peer-b:sink:dev-2", el: sinkEl },
          ]}
        />
        <WireLayer
          boardRef={{ current: makeBoardEl() }}
          streams={[makeStream(1)]}
          selectedId={null}
          onSelect={vi.fn()}
          drag={inactiveDrag}
        />
      </PortRegistryProvider>,
    );

    await waitFor(() => {
      const goldPaths = Array.from(container.querySelectorAll("path")).filter(
        (p) => p.getAttribute("stroke") === "var(--color-gold)",
      );
      expect(goldPaths).toHaveLength(0);
    });
  });
});
