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
import { useDevices } from "@/hooks/useDevices";
import { useSnapshot } from "@/hooks/useSnapshot";
import { usePeers } from "@/hooks/usePeers";
import { useDisconnect } from "@/hooks/useConnection";
import { useUiStore } from "@/stores/ui";
import { Skeleton } from "@/components/ui/skeleton";
import type { StreamSnapshot } from "@/bindings";

function useRemoteName(remotePeerId: string | undefined): string {
  const { data: peers } = usePeers();
  if (!remotePeerId) return "Remoto";
  const match = peers?.find((p) => p.peer_id === remotePeerId);
  if (match) return match.peer_name;
  return remotePeerId.slice(0, 8);
}

function deriveRemoteDevices(streams: StreamSnapshot[], remotePeerId: string) {
  const sourceMap = new Map<string, string>();
  const sinkMap = new Map<string, string>();

  sinkMap.set("default", "Padrão");

  for (const s of streams) {
    if (s.source_peer === remotePeerId) {
      sourceMap.set(s.source_device, s.source_device);
    }
    if (s.sink_peer === remotePeerId) {
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
  const incoming = useUiStore((s) => s.incoming);

  const [modalOpen, setModalOpen] = useState(false);

  const session = snapshots?.[0] ?? null;
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

  const { remoteSources, remoteSinks } = remotePeerId
    ? deriveRemoteDevices(streams, remotePeerId)
    : { remoteSources: [], remoteSinks: [] };

  const wiredPortIds = buildWiredPortIds(streams);
  const portColor = portColorFromStreams(streams);

  const { onPortActivate, hint } = useWiring();

  useTrayHealth(snapshots);

  useEffect(() => {
    if (incoming) {
      setModalOpen(true);
    }
  }, [incoming]);

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
        <div className="flex-none min-h-[96px] bg-[#161618] border-t border-line" />
      </div>
    );
  }

  return (
    <PortRegistryProvider>
      <div className="flex flex-col h-full">
        <div
          ref={boardRef}
          className="relative flex flex-1 items-center justify-center gap-[120px] bg-board"
          style={{
            backgroundImage:
              "repeating-linear-gradient(to right, transparent, transparent 39px, rgba(255,255,255,0.025) 39px, rgba(255,255,255,0.025) 40px)",
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

          {hint && (
            <div className="absolute bottom-3 left-1/2 -translate-x-1/2 z-10 bg-surface border border-line-2 rounded-[2px] px-3 py-[5px] text-[11.5px] text-ink-2 pointer-events-none">
              {hint}
            </div>
          )}
        </div>

        <ChannelDock sessionId={session?.id ?? null} streams={streams} />
      </div>

      <ConnectModal open={modalOpen} onOpenChange={setModalOpen} />
    </PortRegistryProvider>
  );
}
