import { useMutation, useQueryClient } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useOpenStream = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      sessionId,
      sourceDeviceId,
      sourceIsSystem,
      sinkPeerId,
      sinkDeviceId,
      bitrate,
    }: {
      sessionId: string;
      sourceDeviceId: string;
      sourceIsSystem: boolean;
      sinkPeerId: string;
      sinkDeviceId: string;
      bitrate?: number | null;
    }) =>
      unwrap(
        commands.openStream(
          sessionId,
          sourceDeviceId,
          sourceIsSystem,
          sinkPeerId,
          sinkDeviceId,
          bitrate ?? null,
        ),
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
  });
};

export const useCloseStream = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      sessionId,
      streamId,
    }: {
      sessionId: string;
      streamId: number;
    }) => unwrap(commands.closeStream(sessionId, streamId)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
  });
};

export const useStreamControl = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      sessionId,
      streamId,
      action,
      value,
    }: {
      sessionId: string;
      streamId: number;
      action: string;
      value?: number | null;
    }) =>
      unwrap(
        commands.streamControl(sessionId, streamId, action, value ?? null),
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["snapshot"] });
    },
  });
};
