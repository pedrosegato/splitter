import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { commands, unwrap } from "@/lib/api";

export const useStreamRequests = () =>
  useQuery({
    queryKey: ["streamRequests"],
    queryFn: () => unwrap(commands.pendingStreamRequests()),
  });

export const useAcceptStreamRequest = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ requestId }: { requestId: string }) =>
      unwrap(commands.acceptStreamRequest(requestId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["streamRequests"] });
    },
  });
};

export const useRejectStreamRequest = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ requestId }: { requestId: string }) =>
      unwrap(commands.rejectStreamRequest(requestId)),
    onError: (err: Error) => {
      toast.error(err.message);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["streamRequests"] });
    },
  });
};
