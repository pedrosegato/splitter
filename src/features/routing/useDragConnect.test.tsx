import { describe, it, expect, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { PortRegistryProvider } from "./usePortRegistry";
import { useDragConnect } from "./useDragConnect";

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
