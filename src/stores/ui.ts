import { create } from "zustand";

import type { PairingCode, StreamStat } from "@/bindings";

const HISTORY_CAP = 60;

export type StreamHistory = { rtt: number[]; loss: number[]; kbps: number[] };

function cappedAppend(arr: number[], value: number): number[] {
  const next = [...arr, value];
  return next.length > HISTORY_CAP ? next.slice(next.length - HISTORY_CAP) : next;
}

export function pushStatsHistory(
  prev: Record<number, StreamHistory>,
  tick: StreamStat[],
): Record<number, StreamHistory> {
  const next = { ...prev };
  for (const s of tick) {
    const existing = next[s.stream_id] ?? { rtt: [], loss: [], kbps: [] };
    next[s.stream_id] = {
      rtt: cappedAppend(existing.rtt, s.rtt_ms),
      loss: cappedAppend(existing.loss, s.loss_pct),
      kbps: cappedAppend(existing.kbps, s.kbps_sent + s.kbps_received),
    };
  }
  return next;
}

interface UiState {
  activeTab: "routing" | "stats";
  selectedStreamId: number | null;
  stats: StreamStat[];
  statsHistory: Record<number, StreamHistory>;
  knownNames: Record<string, string>;
  /** Set while dialing a first-time pairing. */
  pairing: PairingCode | null;
  setTab: (t: UiState["activeTab"]) => void;
  selectStream: (id: number | null) => void;
  pushStats: (tick: StreamStat[]) => void;
  rememberNames: (names: Record<string, string>) => void;
  setPairing: (pairing: PairingCode | null) => void;
}

export const useUiStore = create<UiState>((set) => ({
  activeTab: "routing",
  selectedStreamId: null,
  stats: [],
  statsHistory: {},
  knownNames: {},
  pairing: null,
  setTab: (activeTab) => set({ activeTab }),
  selectStream: (selectedStreamId) => set({ selectedStreamId }),
  pushStats: (tick) =>
    set((state) => ({
      stats: tick,
      statsHistory: pushStatsHistory(state.statsHistory, tick),
    })),
  rememberNames: (names) => set((state) => ({ knownNames: { ...state.knownNames, ...names } })),
  setPairing: (pairing) => set({ pairing }),
}));
