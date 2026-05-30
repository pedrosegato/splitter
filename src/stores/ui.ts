import { create } from "zustand";
import type { StreamStat } from "@/bindings";

type Arm = { peerId: string; deviceId: string } | null;
type Incoming = { peerId: string; peerName: string } | null;

interface UiState {
  activeTab: "routing" | "stats";
  selectedStreamId: number | null;
  arm: Arm;
  stats: StreamStat[];
  incoming: Incoming;
  setTab: (t: UiState["activeTab"]) => void;
  selectStream: (id: number | null) => void;
  armSource: (peerId: string, deviceId: string) => void;
  clearArm: () => void;
  setStats: (s: StreamStat[]) => void;
  setIncoming: (i: Incoming) => void;
}

export const useUiStore = create<UiState>((set) => ({
  activeTab: "routing",
  selectedStreamId: null,
  arm: null,
  stats: [],
  incoming: null,
  setTab: (activeTab) => set({ activeTab }),
  selectStream: (selectedStreamId) => set({ selectedStreamId }),
  armSource: (peerId, deviceId) => set({ arm: { peerId, deviceId } }),
  clearArm: () => set({ arm: null }),
  setStats: (stats) => set({ stats }),
  setIncoming: (incoming) => set({ incoming }),
}));
