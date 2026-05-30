import { useMutation, useQueryClient } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useConnectPeer = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      host,
      port,
      peerId,
    }: {
      host: string;
      port: number;
      peerId: string | null;
    }) => unwrap(commands.connectPeer(host, port, peerId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["peers"] });
      queryClient.invalidateQueries({ queryKey: ["pending"] });
    },
  });
};

export const useAcceptPending = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ index }: { index: number }) =>
      unwrap(commands.acceptPending(index)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["pending"] });
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
  });
};

export const useDisconnect = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ sessionId }: { sessionId: string }) =>
      unwrap(commands.disconnect(sessionId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
  });
};

export const useOpenSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ remotePeerId }: { remotePeerId: string }) =>
      unwrap(commands.openSession(remotePeerId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
  });
};
