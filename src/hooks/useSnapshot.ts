import { useQuery } from "@tanstack/react-query";
import { commands, unwrap } from "@/lib/api";

export const useSnapshot = () =>
  useQuery({
    queryKey: ["snapshot"],
    queryFn: () => unwrap(commands.snapshot()),
    // Backstop only: internal stream→Error/recovery transitions (splitter-core) emit no SnapshotChanged yet; events drive normal updates. Remove once that event exists.
    refetchInterval: 15000,
  });
