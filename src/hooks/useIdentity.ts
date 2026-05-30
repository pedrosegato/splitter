import { useQuery } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useIdentity = () =>
  useQuery({ queryKey: ["identity"], queryFn: () => unwrap(commands.identity()), staleTime: Infinity });
