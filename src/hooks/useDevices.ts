import { useQuery } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useDevices = () =>
  useQuery({
    queryKey: ["devices"],
    queryFn: () => unwrap(commands.listDevices()),
  });
