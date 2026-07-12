import { useEffect, useRef } from "react";
import { commands } from "@/lib/api";
import { useUiStore } from "@/stores/ui";
import type { SessionSnapshot } from "@/bindings";

type TrayState = "idle" | "active" | "degraded" | "error";

function deriveTrayState(
  snapshots: SessionSnapshot[] | undefined,
  hasDegraded: boolean,
): TrayState {
  const sessions = snapshots ?? [];
  if (sessions.length === 0) return "idle";

  const streams = sessions.flatMap((s) => s.streams);
  if (streams.length === 0) return "active";

  const hasError = streams.some((s) => s.state === "error");
  if (hasError) return "error";

  if (hasDegraded) return "degraded";

  return "active";
}

export function useTrayHealth(snapshots: SessionSnapshot[] | undefined): void {
  const hasDegraded = useUiStore((s) => s.stats.some((stat) => stat.loss_pct > 5));
  const prevStateRef = useRef<TrayState | null>(null);

  const derived = deriveTrayState(snapshots, hasDegraded);

  useEffect(() => {
    if (derived === prevStateRef.current) return;
    prevStateRef.current = derived;
    commands.setTrayState(derived).catch(() => {});
  }, [derived]);
}
