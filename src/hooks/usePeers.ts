import { useQuery } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const usePeers = () =>
  useQuery({
    queryKey: ["peers"],
    queryFn: () => unwrap(commands.discoveredPeers()),
  });

export const usePendingPeers = () =>
  useQuery({
    queryKey: ["pending"],
    queryFn: () => unwrap(commands.pendingPeers()),
    // Backstop only: pending pairing is event-driven via incomingSession; this covers any missed signaling event. Remove once coverage is proven complete.
    refetchInterval: 15000,
  });
