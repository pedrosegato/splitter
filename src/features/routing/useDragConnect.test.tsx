import { describe, it, expect, vi, afterEach } from "vitest";
import { renderHook, act, render } from "@testing-library/react";
import { useEffect, useRef } from "react";
import { PortRegistryProvider, usePortRegistry } from "./usePortRegistry";
import { useDragConnect } from "./useDragConnect";
import type { PortRef } from "./resolveConnection";

function wrapper({ children }: { children: React.ReactNode }) {
  return <PortRegistryProvider>{children}</PortRegistryProvider>;
}

describe("useDragConnect", () => {
  it("starts inactive", () => {
    const boardRef = { current: document.createElement("div") };
    const { result } = renderHook(
      () => useDragConnect({ boardRef, onConnect: vi.fn() }),
      { wrapper },
    );
    expect(result.current.drag.active).toBe(false);
  });

  it("activates on startDrag and tracks the origin port", () => {
    const boardRef = { current: document.createElement("div") };
    const { result } = renderHook(
      () => useDragConnect({ boardRef, onConnect: vi.fn() }),
      { wrapper },
    );
    act(() => {
      result.current.startDrag(
        { peerId: "A", deviceId: "mic", kind: "src" },
        { clientX: 10, clientY: 10, pointerId: 1 } as unknown as React.PointerEvent,
      );
    });
    expect(result.current.drag.active).toBe(true);
    expect(result.current.drag.from?.peerId).toBe("A");
  });
});

describe("useDragConnect finish", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = "";
  });

  function setup(onConnect: (a: PortRef, b: PortRef) => void) {
    const origin: PortRef = { peerId: "A", deviceId: "mic", kind: "src" };
    const target: PortRef = { peerId: "B", deviceId: "spk", kind: "sink" };
    const api: {
      startDrag?: (o: PortRef, e: React.PointerEvent) => void;
      originEl?: HTMLElement;
      targetEl?: HTMLElement;
    } = {};

    function Harness() {
      const registry = usePortRegistry();
      const boardRef = useRef(document.createElement("div"));
      const { startDrag } = useDragConnect({ boardRef, onConnect });
      useEffect(() => {
        const originEl = document.createElement("button");
        originEl.dataset.portId = "A:src:mic";
        const targetEl = document.createElement("button");
        targetEl.dataset.portId = "B:sink:spk";
        document.body.append(originEl, targetEl);
        registry.register("A:src:mic", originEl, origin);
        registry.register("B:sink:spk", targetEl, target);
        api.startDrag = startDrag;
        api.originEl = originEl;
        api.targetEl = targetEl;
      }, [registry, startDrag]);
      return null;
    }

    render(
      <PortRegistryProvider>
        <Harness />
      </PortRegistryProvider>,
    );
    return { api, origin, target };
  }

  it("does not connect when released on the origin port", () => {
    const onConnect = vi.fn();
    const { api } = setup(onConnect);
    document.elementFromPoint = () => api.originEl!;

    act(() => {
      api.startDrag!(
        { peerId: "A", deviceId: "mic", kind: "src" },
        { clientX: 5, clientY: 5, pointerId: 1 } as unknown as React.PointerEvent,
      );
    });
    act(() => {
      window.dispatchEvent(new PointerEvent("pointerup", { clientX: 5, clientY: 5 }));
    });

    expect(onConnect).not.toHaveBeenCalled();
  });

  it("connects when released on a different port", () => {
    const onConnect = vi.fn();
    const { api, origin, target } = setup(onConnect);
    document.elementFromPoint = () => api.targetEl!;

    act(() => {
      api.startDrag!(
        { peerId: "A", deviceId: "mic", kind: "src" },
        { clientX: 5, clientY: 5, pointerId: 1 } as unknown as React.PointerEvent,
      );
    });
    act(() => {
      window.dispatchEvent(new PointerEvent("pointerup", { clientX: 60, clientY: 60 }));
    });

    expect(onConnect).toHaveBeenCalledWith(origin, target);
  });
});
