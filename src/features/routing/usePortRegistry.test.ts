import { createElement, type ReactNode } from "react";
import { describe, it, expect } from "vitest";
import { renderHook } from "@testing-library/react";
import { PortRegistryProvider, usePortRegistry } from "./usePortRegistry";

function wrapper({ children }: { children: ReactNode }) {
  return createElement(PortRegistryProvider, null, children);
}

describe("usePortRegistry", () => {
  it("registers and retrieves an element with the 2-arg form", () => {
    const { result } = renderHook(() => usePortRegistry(), { wrapper });
    const el = document.createElement("div");

    result.current.register("p1", el);

    expect(result.current.get("p1")).toBe(el);
    expect(result.current.getRef("p1")).toBeNull();
  });

  it("removes the element and its ref when registered with null", () => {
    const { result } = renderHook(() => usePortRegistry(), { wrapper });
    const el = document.createElement("div");

    result.current.register("p1", el, { peerId: "A", deviceId: "mic", kind: "src" });
    expect(result.current.get("p1")).toBe(el);
    expect(result.current.getRef("p1")).toEqual({ peerId: "A", deviceId: "mic", kind: "src" });

    result.current.register("p1", null);

    expect(result.current.get("p1")).toBeUndefined();
    expect(result.current.getRef("p1")).toBeNull();
  });

  it("stores the optional PortRef alongside the element with the 3-arg form", () => {
    const { result } = renderHook(() => usePortRegistry(), { wrapper });
    const el = document.createElement("div");
    const ref = { peerId: "B", deviceId: "speaker", kind: "sink" as const };

    result.current.register("p2", el, ref);

    expect(result.current.get("p2")).toBe(el);
    expect(result.current.getRef("p2")).toEqual(ref);
  });

  it("getRef returns null for an unknown portId", () => {
    const { result } = renderHook(() => usePortRegistry(), { wrapper });

    expect(result.current.getRef("unknown")).toBeNull();
  });

  it("throws when used outside the provider", () => {
    expect(() => renderHook(() => usePortRegistry())).toThrow("PortRegistry missing");
  });
});
