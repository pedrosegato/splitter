import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { commands, unwrap } from "@/lib/api";

export const useSetDeviceName = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => unwrap(commands.setDeviceName(name)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["identity"] });
      queryClient.invalidateQueries({ queryKey: ["peers"] });
    },
    onError: (err: Error) => {
      toast.error(err.message);
    },
  });
};
