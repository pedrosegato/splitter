import { render, act } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { usePortRegistry, PortRegistryProvider } from "./usePortRegistry";
import { Port } from "./Port";

function RegistryProbe({
  portId,
  onRegistry,
}: {
  portId: string;
  onRegistry: (get: (id: string) => HTMLElement | undefined) => void;
}) {
  const registry = usePortRegistry();
  onRegistry(registry.get.bind(registry));
  return null;
}

describe("Port + PortRegistryProvider", () => {
  it("registers the port element in the registry after mount", () => {
    let getEl: ((id: string) => HTMLElement | undefined) | undefined;

    const { getByRole } = render(
      <PortRegistryProvider>
        <Port peerId="peer-1" kind="src" deviceId="dev-a" />
        <RegistryProbe
          portId="peer-1:src:dev-a"
          onRegistry={(fn) => {
            getEl = fn;
          }}
        />
      </PortRegistryProvider>,
    );

    const button = getByRole("button");
    expect(getEl).toBeDefined();
    expect(getEl!("peer-1:src:dev-a")).toBe(button);
  });

  it("removes the port element from the registry after unmount", () => {
    let getEl: ((id: string) => HTMLElement | undefined) | undefined;

    const { unmount, getByRole: _getByRole } = render(
      <PortRegistryProvider>
        <Port peerId="peer-2" kind="sink" deviceId="dev-b" />
        <RegistryProbe
          portId="peer-2:sink:dev-b"
          onRegistry={(fn) => {
            getEl = fn;
          }}
        />
      </PortRegistryProvider>,
    );

    expect(getEl!("peer-2:sink:dev-b")).toBeInstanceOf(HTMLButtonElement);

    act(() => {
      unmount();
    });

    expect(getEl!("peer-2:sink:dev-b")).toBeUndefined();
  });

  it("constructs the portId as peerId:kind:deviceId", () => {
    let getEl: ((id: string) => HTMLElement | undefined) | undefined;

    render(
      <PortRegistryProvider>
        <Port peerId="abc" kind="src" deviceId="xyz" />
        <RegistryProbe
          portId="abc:src:xyz"
          onRegistry={(fn) => {
            getEl = fn;
          }}
        />
      </PortRegistryProvider>,
    );

    expect(getEl!("abc:src:xyz")).toBeInstanceOf(HTMLButtonElement);
    expect(getEl!("abc:sink:xyz")).toBeUndefined();
  });

  it("throws when usePortRegistry is used outside provider", () => {
    const originalError = console.error;
    console.error = () => {};
    expect(() =>
      render(
        <RegistryProbe portId="x" onRegistry={() => {}} />,
      ),
    ).toThrow("PortRegistry missing");
    console.error = originalError;
  });

  it("applies wired styles when wired and color are provided", () => {
    const { getByRole } = render(
      <PortRegistryProvider>
        <Port peerId="p1" kind="src" deviceId="d1" wired color="#e3251f" />
      </PortRegistryProvider>,
    );

    const button = getByRole("button");
    expect(button.style.backgroundColor).toBe("rgb(227, 37, 31)");
    expect(button.style.borderColor).toBe("rgb(227, 37, 31)");
  });

  it("does not apply inline styles when not wired", () => {
    const { getByRole } = render(
      <PortRegistryProvider>
        <Port peerId="p1" kind="src" deviceId="d1" color="#e3251f" />
      </PortRegistryProvider>,
    );

    const button = getByRole("button");
    expect(button.style.backgroundColor).toBe("");
    expect(button.style.borderColor).toBe("");
  });

  it("calls onActivate with correct args on click", async () => {
    let captured: unknown;
    const { getByRole } = render(
      <PortRegistryProvider>
        <Port
          peerId="p9"
          kind="sink"
          deviceId="d9"
          onActivate={(...args) => {
            captured = args;
          }}
        />
      </PortRegistryProvider>,
    );

    act(() => {
      getByRole("button").click();
    });

    expect(captured).toEqual(["p9:sink:d9", "sink", "p9", "d9"]);
  });

  it("exposes a data-port-id for hit-testing", () => {
    const { getByRole } = render(
      <PortRegistryProvider>
        <Port peerId="A" kind="src" deviceId="mic" />
      </PortRegistryProvider>,
    );

    expect(getByRole("button")).toHaveAttribute("data-port-id", "A:src:mic");
  });

  it("still calls onActivate on click (a11y path)", () => {
    let captured: unknown;
    const { getByRole } = render(
      <PortRegistryProvider>
        <Port
          peerId="A"
          kind="src"
          deviceId="mic"
          onActivate={(...args) => {
            captured = args;
          }}
        />
      </PortRegistryProvider>,
    );

    act(() => {
      getByRole("button").click();
    });

    expect(captured).toEqual(["A:src:mic", "src", "A", "mic"]);
  });
});
