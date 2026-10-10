import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { commands, unwrap } from "@/lib/api";
import { useUiStore } from "@/stores/ui";

export const useConnectPeer = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ host, port, peerId }: { host: string; port: number; peerId: string | null }) =>
      unwrap(commands.connectPeer(host, port, peerId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["peers"] });
      queryClient.invalidateQueries({ queryKey: ["pending"] });
      toast.success("Peer pareado");
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
    onSettled: () => useUiStore.getState().setPairing(null),
  });
};

export const useAcceptPending = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ peerId }: { peerId: string }) => unwrap(commands.acceptPending(peerId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["pending"] });
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};

export const useRejectPending = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ peerId }: { peerId: string }) => unwrap(commands.rejectPending(peerId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["pending"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};

export const useDisconnect = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ sessionId }: { sessionId: string }) => unwrap(commands.disconnect(sessionId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
      toast.success("Desconectado");
    },
    onError: (err: Error) => {
      toast.error(err.message);
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
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};
