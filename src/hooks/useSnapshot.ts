import { useQuery } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useSnapshot = () =>
  useQuery({
    queryKey: ["snapshot"],
    queryFn: () => unwrap(commands.snapshot()),
    refetchInterval: 3000,
  });
