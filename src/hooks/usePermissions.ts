import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { commands } from "@/lib/api";

export const usePermissions = () =>
  useQuery({
    queryKey: ["permissions"],
    queryFn: () => commands.permissionStatus(),
  });

export const useRequestPermission = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (kind: string) => commands.requestPermission(kind),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["permissions"] }),
  });
};
