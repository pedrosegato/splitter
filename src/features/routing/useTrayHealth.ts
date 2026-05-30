import { useEffect, useRef } from "react";
import { commands } from "@/lib/api";
import { useUiStore } from "@/stores/ui";
import type { SessionSnapshot } from "@/bindings";

type TrayState = "idle" | "active" | "degraded" | "error";

function deriveTrayState(
  snapshots: SessionSnapshot[] | undefined,
  latestStats: ReturnType<typeof useUiStore.getState>["stats"],
): TrayState {
  const sessions = snapshots ?? [];
  if (sessions.length === 0) return "idle";

  const streams = sessions.flatMap((s) => s.streams);
  if (streams.length === 0) return "active";

  const hasError = streams.some((s) => s.state === "error");
  if (hasError) return "error";

  const hasDegraded = latestStats.some((stat) => stat.loss_pct > 5);
  if (hasDegraded) return "degraded";

  return "active";
}

export function useTrayHealth(snapshots: SessionSnapshot[] | undefined): void {
  const stats = useUiStore((s) => s.stats);
  const prevStateRef = useRef<TrayState | null>(null);

  const derived = deriveTrayState(snapshots, stats);

  useEffect(() => {
    if (derived === prevStateRef.current) return;
    prevStateRef.current = derived;
    commands.setTrayState(derived).catch(() => {});
  }, [derived]);
}
