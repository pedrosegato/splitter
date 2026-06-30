import type { QueryClient } from "@tanstack/react-query";
import { events } from "@/lib/api";
import { useUiStore } from "@/stores/ui";

export async function mountEventBridge(qc: QueryClient): Promise<() => void> {
  const unlisten = await Promise.all([
    events.peersChanged.listen((e) => {
      const names: Record<string, string> = {};
      for (const p of e.payload) names[p.peer_id] = p.peer_name;
      useUiStore.getState().rememberNames(names);
      qc.invalidateQueries({ queryKey: ["peers"] });
    }),
    events.incomingSession.listen((e) => {
      useUiStore.getState().rememberNames({ [e.payload.peer_id]: e.payload.peer_name });
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
