import { useQuery } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useDevices = () =>
  useQuery({
    queryKey: ["devices"],
    queryFn: () => unwrap(commands.listDevices()),
  });

export const usePeerDevices = (peerId: string | undefined) =>
  useQuery({
    queryKey: ["peerDevices", peerId],
    queryFn: () => unwrap(commands.peerDevices(peerId!)),
    enabled: !!peerId,
  });
