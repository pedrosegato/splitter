import { useEffect, useMemo, useRef, useState } from "react";
import { motion } from "motion/react";

import type { DeviceDescriptor, StreamSnapshot } from "@/bindings";
import { Skeleton } from "@/components/ui/skeleton";
import { ConnectModal } from "@/features/connect/ConnectModal";
import { useActiveSession } from "@/hooks/useActiveSession";
import { useDisconnect } from "@/hooks/useConnection";
import { useDevices } from "@/hooks/useDevices";
import { useIdentity } from "@/hooks/useIdentity";
import { usePeers } from "@/hooks/usePeers";
import { useSnapshot } from "@/hooks/useSnapshot";
import { variants } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { ChannelDock } from "./ChannelDock";
import { MachinePanel, panelCardClass } from "./MachinePanel";
import { useDragConnect } from "./useDragConnect";
import { PortRegistryProvider } from "./usePortRegistry";
import { useTrayHealth } from "./useTrayHealth";
import { streamColor } from "./useWireGeometry";
import { useWiring } from "./useWiring";
import { WireLayer } from "./WireLayer";

function useRemoteName(remotePeerId: string | undefined, sessionName: string | undefined): string {
  const { data: peers } = usePeers();
  const knownNames = useUiStore((s) => s.knownNames);
  const rememberNames = useUiStore((s) => s.rememberNames);
  const match = remotePeerId ? peers?.find((p) => p.peer_id === remotePeerId) : undefined;
  const resolvedName = sessionName || match?.peer_name;

  useEffect(() => {
    if (remotePeerId && resolvedName) rememberNames({ [remotePeerId]: resolvedName });
  }, [remotePeerId, resolvedName, rememberNames]);

  if (!remotePeerId) return "Remoto";
  if (resolvedName) return resolvedName;
  if (knownNames[remotePeerId]) return knownNames[remotePeerId];
  return remotePeerId.slice(0, 8);
}

function srcPortId(s: StreamSnapshot): string {
  return `${s.source_peer}:src:${s.source_device}`;
}

function sinkPortId(s: StreamSnapshot): string {
  return `${s.sink_peer}:sink:${s.sink_device}`;
}

function toDeviceOptions(
  devices: { id: string; name: string; kind: string }[],
  kind: string,
): { id: string; name: string }[] {
  return devices.filter((d) => d.kind === kind).map((d) => ({ id: d.id, name: d.name }));
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
    ids.add(srcPortId(s));
    ids.add(sinkPortId(s));
  }
  return ids;
}

function buildPortColorMap(streams: StreamSnapshot[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const s of streams) {
    const color = streamColor(s.id);
    const src = srcPortId(s);
    const sink = sinkPortId(s);
    if (!map.has(src)) map.set(src, color);
    if (!map.has(sink)) map.set(sink, color);
  }
  return map;
}

export function RoutingBoard() {
  return (
    <PortRegistryProvider>
      <RoutingBoardContent />
    </PortRegistryProvider>
  );
}

function RoutingBoardContent() {
  const boardRef = useRef<HTMLDivElement | null>(null);

  const { data: identity } = useIdentity();
  const { data: devices, isLoading: devicesLoading } = useDevices();
  const { data: snapshots, isLoading: snapshotLoading } = useSnapshot();
  const disconnect = useDisconnect();
  const isLoading = devicesLoading || snapshotLoading;

  const selectedStreamId = useUiStore((s) => s.selectedStreamId);
  const selectStream = useUiStore((s) => s.selectStream);

  const [modalOpen, setModalOpen] = useState(false);

  const { session, streams, remotePeerId, peerDevices } = useActiveSession();
  const connected = !!session;

  const selfPeerId = identity?.peer_id ?? "";
  const selfName = identity?.peer_name ?? "Este Mac";

  const selfSinks = useMemo(() => toDeviceOptions(devices ?? [], "Output"), [devices]);

  const selfSources = useMemo(
    () =>
      (devices ?? [])
        .filter((d) => d.kind === "Input" || d.kind === "SystemAudio")
        .map((d) => ({ id: d.id, name: d.name })),
    [devices],
  );

  const remoteName = useRemoteName(remotePeerId, session?.remote_peer_name);

  const { remoteSources, remoteSinks } = useMemo(
    () =>
      remotePeerId
        ? deriveRemoteDevices(streams, remotePeerId, peerDevices ?? [])
        : { remoteSources: [], remoteSinks: [] },
    [streams, remotePeerId, peerDevices],
  );

  const wiredPortIds = useMemo(() => buildWiredPortIds(streams), [streams]);

  const portColorMap = useMemo(() => buildPortColorMap(streams), [streams]);
  const portColor = useMemo(() => (id: string) => portColorMap.get(id), [portColorMap]);

  const { onPortConnect } = useWiring();
  const { drag, startDrag } = useDragConnect({ boardRef, onConnect: onPortConnect });

  useTrayHealth(snapshots);

  if (isLoading) {
    return (
      <div className="flex h-full flex-col">
        <div className="bg-board relative flex flex-1 items-center justify-center gap-[120px]">
          {[0, 1].map((i) => (
            <div key={i} className={cn(panelCardClass, "flex flex-col gap-2 p-3")}>
              <Skeleton className="bg-line-2 h-4 w-2/3" />
              <Skeleton className="bg-line-2 h-3 w-full" />
              <Skeleton className="bg-line-2 h-3 w-4/5" />
              <Skeleton className="bg-line-2 h-3 w-3/4" />
            </div>
          ))}
        </div>
        <div className="bg-elev-0 border-line min-h-[96px] flex-none border-t" />
      </div>
    );
  }

  return (
    <>
      <div className="flex h-full flex-col">
        <div
          ref={boardRef}
          onClick={(e) => {
            if (e.target === e.currentTarget) selectStream(null);
          }}
          className="bg-board relative flex flex-1 items-center justify-center gap-[120px]"
          style={{
            backgroundImage:
              "repeating-linear-gradient(to right, transparent, transparent 39px, var(--grid-line) 39px, var(--grid-line) 40px)",
          }}
        >
          <motion.div variants={variants.fade} initial="hidden" animate="show">
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
              onDragStart={startDrag}
              dragFrom={drag.from}
              dragActive={drag.active}
            />
          </motion.div>

          <motion.div variants={variants.fade} initial="hidden" animate="show">
            <MachinePanel
              peerId={remotePeerId ?? "remote"}
              name={remoteName}
              side="right"
              connected={connected}
              sinks={remoteSinks}
              sources={remoteSources}
              wiredPortIds={wiredPortIds}
              portColor={portColor}
              onDragStart={startDrag}
              dragFrom={drag.from}
              dragActive={drag.active}
              onConnectClick={() => setModalOpen(true)}
              onDisconnect={
                session ? () => disconnect.mutate({ sessionId: session.id }) : undefined
              }
            />
          </motion.div>

          <WireLayer
            boardRef={boardRef}
            streams={streams}
            selectedId={selectedStreamId}
            onSelect={selectStream}
            drag={drag}
          />
        </div>

        <ChannelDock sessionId={session?.id ?? null} streams={streams} />
      </div>

      <ConnectModal open={modalOpen} onOpenChange={setModalOpen} />
    </>
  );
}
