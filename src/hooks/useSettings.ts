import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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
  });
};
