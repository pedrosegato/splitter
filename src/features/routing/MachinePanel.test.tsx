import { render } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { PortRegistryProvider } from "./usePortRegistry";
import { MachinePanel } from "./MachinePanel";

const sinks = [
  { id: "sink-a", name: "Fones MCHOSE V9" },
  { id: "sink-b", name: "Alto-falantes" },
];

const sources = [
  { id: "src-a", name: "Microfone" },
  { id: "src-b", name: "Sistema" },
];

function wrap(ui: React.ReactElement) {
  return render(<PortRegistryProvider>{ui}</PortRegistryProvider>);
}

describe("MachinePanel", () => {
  it("renders DESTINOS and FONTES labels with device names for a connected self panel", () => {
    const { getByText, getAllByRole } = wrap(
      <MachinePanel
        peerId="local"
        name="Este Mac"
        side="left"
        isSelf
        connected
        sinks={sinks}
        sources={sources}
      />,
    );

    expect(getByText("DESTINOS")).toBeTruthy();
    expect(getByText("FONTES")).toBeTruthy();

    expect(getByText("Fones MCHOSE V9")).toBeTruthy();
    expect(getByText("Alto-falantes")).toBeTruthy();
    expect(getByText("Microfone")).toBeTruthy();
    expect(getByText("Sistema")).toBeTruthy();

    const ports = getAllByRole("button");
    expect(ports).toHaveLength(4);
  });

  it("shows ESTE PC tag for self panel", () => {
    const { getByText } = wrap(
      <MachinePanel
        peerId="local"
        name="Este Mac"
        side="left"
        isSelf
        connected
        sinks={sinks}
        sources={sources}
      />,
    );

    expect(getByText("ESTE PC")).toBeTruthy();
  });

  it("renders only the connect slot for right side when not connected", () => {
    const { getByRole, queryByText } = wrap(
      <MachinePanel
        peerId="remote"
        name="Studio PC"
        side="right"
        connected={false}
        sinks={[]}
        sources={[]}
      />,
    );

    const btn = getByRole("button", { name: /Conectar máquina/i });
    expect(btn).toBeTruthy();

    expect(queryByText("DESTINOS")).toBeNull();
    expect(queryByText("FONTES")).toBeNull();
  });

  it("calls onConnectClick when the connect button is clicked", () => {
    let clicked = false;
    const { getByRole } = wrap(
      <MachinePanel
        peerId="remote"
        name="Studio PC"
        side="right"
        connected={false}
        sinks={[]}
        sources={[]}
        onConnectClick={() => {
          clicked = true;
        }}
      />,
    );

    getByRole("button", { name: /Conectar máquina/i }).click();
    expect(clicked).toBe(true);
  });

  it("shows latency for connected remote panel", () => {
    const { getByText } = wrap(
      <MachinePanel
        peerId="remote"
        name="Studio PC"
        side="right"
        connected
        sinks={sinks}
        sources={sources}
        latencyMs={4}
      />,
    );

    expect(getByText("4 ms")).toBeTruthy();
  });

  it("shows disconnect button for remote panel and calls onDisconnect", () => {
    let disconnected = false;
    const { getByTitle } = wrap(
      <MachinePanel
        peerId="remote"
        name="Studio PC"
        side="right"
        connected
        sinks={sinks}
        sources={sources}
        onDisconnect={() => {
          disconnected = true;
        }}
      />,
    );

    const btn = getByTitle("desconectar");
    expect(btn).toBeTruthy();
    btn.click();
    expect(disconnected).toBe(true);
  });

  it("marks ports as wired when portId is in wiredPortIds", () => {
    const wiredPortIds = new Set(["local:sink:sink-a"]);
    const portColor = (id: string) =>
      id === "local:sink:sink-a" ? "#e3251f" : undefined;

    const { getAllByRole } = wrap(
      <MachinePanel
        peerId="local"
        name="Este Mac"
        side="left"
        isSelf
        connected
        sinks={sinks}
        sources={sources}
        wiredPortIds={wiredPortIds}
        portColor={portColor}
      />,
    );

    const ports = getAllByRole("button");
    expect(ports).toHaveLength(4);

    const wiredPort = ports.find(
      (p) => p.style.backgroundColor === "rgb(227, 37, 31)",
    );
    expect(wiredPort).toBeTruthy();
  });
});
