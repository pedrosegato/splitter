import { useCallback } from "react";
import { motion } from "motion/react";
import { cn } from "@/lib/utils";
import { springs } from "@/lib/motion";
import { usePortRegistry } from "./usePortRegistry";
import type { PortRef } from "./resolveConnection";

type PortProps = {
  peerId: string;
  kind: "src" | "sink";
  deviceId: string;
  wired?: boolean;
  color?: string;
  onDragStart?: (ref: PortRef, e: React.PointerEvent) => void;
  highlighted?: boolean;
  dimmed?: boolean;
};

export function Port({
  peerId,
  kind,
  deviceId,
  wired,
  color,
  onDragStart,
  highlighted,
  dimmed,
}: PortProps) {
  const registry = usePortRegistry();
  const portId = `${peerId}:${kind}:${deviceId}`;

  const refCallback = useCallback(
    (el: HTMLButtonElement | null) => {
      registry.register(portId, el, { peerId, deviceId, kind });
    },
    [registry, portId, peerId, deviceId, kind],
  );

  return (
    <motion.button
      ref={refCallback}
      type="button"
      data-port-id={portId}
      aria-label={`${kind === "src" ? "Source" : "Sink"} port for device ${deviceId} on peer ${peerId}`}
      className={cn(
        "w-3 h-3 rounded-full border-2 cursor-crosshair transition-all duration-100",
        "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-gold/40",
        "border-line-2 bg-board hover:border-gold focus-visible:border-gold",
      )}
      style={wired && color ? { backgroundColor: color, borderColor: color } : undefined}
      animate={{ scale: highlighted ? 1.4 : 1, opacity: dimmed ? 0.25 : 1 }}
      whileHover={{ scale: 1.35 }}
      whileTap={{ scale: 0.9 }}
      transition={springs.snappy}
      onPointerDown={(e) => {
        e.preventDefault();
        onDragStart?.({ peerId, deviceId, kind }, e);
      }}
    />
  );
}
