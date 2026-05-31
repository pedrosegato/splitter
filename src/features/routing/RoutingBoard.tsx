import { useEffect, useRef, useState } from "react";
import { PortRegistryProvider } from "./usePortRegistry";
import { MachinePanel } from "./MachinePanel";
import { WireLayer } from "./WireLayer";
import { ChannelDock } from "./ChannelDock";
import { ConnectModal } from "@/features/connect/ConnectModal";
import { useWiring } from "./useWiring";
import { useTrayHealth } from "./useTrayHealth";
import { streamColor } from "./useWireGeometry";
import { useIdentity } from "@/hooks/useIdentity";
import { useDevices, usePeerDevices } from "@/hooks/useDevices";
import { useSnapshot } from "@/hooks/useSnapshot";
import { usePeers } from "@/hooks/usePeers";
import { useDisconnect } from "@/hooks/useConnection";
import { useUiStore } from "@/stores/ui";
import { Skeleton } from "@/components/ui/skeleton";
import type { StreamSnapshot, DeviceDescriptor } from "@/bindings";

function useRemoteName(remotePeerId: string | undefined): string {
  const { data: peers } = usePeers();
  const knownNames = useUiStore((s) => s.knownNames);
  const rememberNames = useUiStore((s) => s.rememberNames);
  const match = remotePeerId
    ? peers?.find((p) => p.peer_id === remotePeerId)
    : undefined;
  const matchName = match?.peer_name;

  useEffect(() => {
    if (remotePeerId && matchName) rememberNames({ [remotePeerId]: matchName });
  }, [remotePeerId, matchName, rememberNames]);

  if (!remotePeerId) return "Remoto";
  if (matchName) return matchName;
  if (knownNames[remotePeerId]) return knownNames[remotePeerId];
  return remotePeerId.slice(0, 8);
}

function deriveRemoteDevices(
  streams: StreamSnapshot[],
  remotePeerId: string,
  devices: DeviceDescriptor[],
) {
  const sourceMap = new Map<string, string>();
  const sinkMap = new Map<string, string>();

  sinkMap.set("default", "Padrão");

  for (const d of devices) {
    if (d.kind === "Output") {
      sinkMap.set(d.id, d.name);
    } else {
      sourceMap.set(d.id, d.name);
    }
  }

  for (const s of streams) {
    if (s.source_peer === remotePeerId && !sourceMap.has(s.source_device)) {
      sourceMap.set(s.source_device, s.source_device);
    }
    if (s.sink_peer === remotePeerId && !sinkMap.has(s.sink_device)) {
      sinkMap.set(s.sink_device, s.sink_device);
    }
  }

  return {
    remoteSources: Array.from(sourceMap.entries()).map(([id, name]) => ({ id, name })),
    remoteSinks: Array.from(sinkMap.entries()).map(([id, name]) => ({ id, name })),
  };
}

function buildWiredPortIds(streams: StreamSnapshot[]): Set<string> {
  const ids = new Set<string>();
  for (const s of streams) {
    ids.add(`${s.source_peer}:src:${s.source_device}`);
    ids.add(`${s.sink_peer}:sink:${s.sink_device}`);
  }
  return ids;
}

function portColorFromStreams(streams: StreamSnapshot[]) {
  return (portId: string): string | undefined => {
    for (const s of streams) {
      if (
        portId === `${s.source_peer}:src:${s.source_device}` ||
        portId === `${s.sink_peer}:sink:${s.sink_device}`
      ) {
        return streamColor(s.id);
      }
    }
    return undefined;
  };
}

export function RoutingBoard() {
  const boardRef = useRef<HTMLDivElement | null>(null);

  const { data: identity } = useIdentity();
  const { data: devices, isLoading: devicesLoading } = useDevices();
  const { data: snapshots, isLoading: snapshotLoading } = useSnapshot();
  const disconnect = useDisconnect();
  const isLoading = devicesLoading || snapshotLoading;

  const selectedStreamId = useUiStore((s) => s.selectedStreamId);
  const selectStream = useUiStore((s) => s.selectStream);
  const clearArm = useUiStore((s) => s.clearArm);

  const [modalOpen, setModalOpen] = useState(false);

  const session = snapshots?.find((s) => s.state === "active") ?? null;
  const connected = !!session;
  const streams = session?.streams ?? [];

  const selfPeerId = identity?.peer_id ?? "";
  const selfName = identity?.peer_name ?? "Este Mac";

  const selfSinks = (devices ?? [])
    .filter((d) => d.kind === "Output")
    .map((d) => ({ id: d.id, name: d.name }));

  const selfSources = (devices ?? [])
    .filter((d) => d.kind === "Input" || d.kind === "SystemAudio")
    .map((d) => ({ id: d.id, name: d.name }));

  const remotePeerId = session?.remote_peer_id;
  const remoteName = useRemoteName(remotePeerId);
  const { data: peerDevices } = usePeerDevices(remotePeerId);

  const { remoteSources, remoteSinks } = remotePeerId
    ? deriveRemoteDevices(streams, remotePeerId, peerDevices ?? [])
    : { remoteSources: [], remoteSinks: [] };

  const wiredPortIds = buildWiredPortIds(streams);
  const portColor = portColorFromStreams(streams);

  const { onPortActivate } = useWiring();

  useTrayHealth(snapshots);

  if (isLoading) {
    return (
      <div className="flex flex-col h-full">
        <div className="relative flex flex-1 items-center justify-center gap-[120px] bg-board">
          {[0, 1].map((i) => (
            <div key={i} className="w-[262px] bg-surface border border-line rounded-[3px] p-3 flex flex-col gap-2">
              <Skeleton className="h-4 w-2/3 bg-line-2" />
              <Skeleton className="h-3 w-full bg-line-2" />
              <Skeleton className="h-3 w-4/5 bg-line-2" />
              <Skeleton className="h-3 w-3/4 bg-line-2" />
            </div>
          ))}
        </div>
        <div className="flex-none min-h-[96px] bg-elev-0 border-t border-line" />
      </div>
    );
  }

  return (
    <PortRegistryProvider>
      <div className="flex flex-col h-full">
        <div
          ref={boardRef}
          onClick={(e) => {
            if (e.target === e.currentTarget) {
              selectStream(null);
              clearArm();
            }
          }}
          className="relative flex flex-1 items-center justify-center gap-[120px] bg-board"
          style={{
            backgroundImage:
              "repeating-linear-gradient(to right, transparent, transparent 39px, var(--grid-line) 39px, var(--grid-line) 40px)",
          }}
        >
          <MachinePanel
            peerId={selfPeerId}
            name={selfName}
            side="left"
            isSelf
            connected
            sinks={selfSinks}
            sources={selfSources}
            wiredPortIds={wiredPortIds}
            portColor={portColor}
            onPortActivate={onPortActivate}
          />

          <MachinePanel
            peerId={remotePeerId ?? "remote"}
            name={remoteName}
            side="right"
            connected={connected}
            sinks={remoteSinks}
            sources={remoteSources}
            wiredPortIds={wiredPortIds}
            portColor={portColor}
            onPortActivate={onPortActivate}
            onConnectClick={() => setModalOpen(true)}
            onDisconnect={
              session
                ? () => disconnect.mutate({ sessionId: session.id })
                : undefined
            }
          />

          <WireLayer
            boardRef={boardRef}
            streams={streams}
            selectedId={selectedStreamId}
            onSelect={selectStream}
          />
        </div>

        <ChannelDock sessionId={session?.id ?? null} streams={streams} />
      </div>

      <ConnectModal open={modalOpen} onOpenChange={setModalOpen} />
    </PortRegistryProvider>
  );
}
