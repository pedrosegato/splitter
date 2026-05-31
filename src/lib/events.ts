import type { QueryClient } from "@tanstack/react-query";
import { events } from "@/lib/api";
import { useUiStore } from "@/stores/ui";

export async function mountEventBridge(qc: QueryClient): Promise<() => void> {
  const unlisten = await Promise.all([
    events.peersChanged.listen(() => qc.invalidateQueries({ queryKey: ["peers"] })),
    events.incomingSession.listen((e) => {
      useUiStore.getState().setIncoming({
        peerId: e.payload.peer_id,
        peerName: e.payload.peer_name,
      });
      qc.invalidateQueries({ queryKey: ["snapshot"] });
      qc.invalidateQueries({ queryKey: ["pending"] });
    }),
    events.statsTick.listen((e) => useUiStore.getState().pushStats(e.payload)),
    events.peerDisconnected.listen(() => {
      qc.invalidateQueries({ queryKey: ["snapshot"] });
      qc.invalidateQueries({ queryKey: ["peers"] });
    }),
    events.snapshotChanged.listen(() => {
      qc.invalidateQueries({ queryKey: ["snapshot"] });
      qc.invalidateQueries({ queryKey: ["peerDevices"] });
    }),
  ]);
  return () => unlisten.forEach((u) => u());
}
