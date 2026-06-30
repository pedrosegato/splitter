import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { commands, unwrap } from "@/lib/api";

export const useSettings = () =>
  useQuery({
    queryKey: ["settings"],
    queryFn: () => unwrap(commands.settingsGet()),
  });

export const useSetSetting = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) =>
      unwrap(commands.settingsSet(key, value)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};

export const useResetSettings = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(commands.settingsReset()),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};

export const useSetAutostart = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) => unwrap(commands.setAutostart(enabled)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};
