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
    refetchInterval: 1500,
  });
