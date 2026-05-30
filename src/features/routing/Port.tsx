import { useCallback } from "react";
import { cn } from "@/lib/utils";
import { usePortRegistry } from "./usePortRegistry";

type PortProps = {
  peerId: string;
  kind: "src" | "sink";
  deviceId: string;
  wired?: boolean;
  color?: string;
  onActivate?: (
    portId: string,
    kind: "src" | "sink",
    peerId: string,
    deviceId: string,
  ) => void;
};

export function Port({ peerId, kind, deviceId, wired, color, onActivate }: PortProps) {
  const registry = usePortRegistry();
  const portId = `${peerId}:${kind}:${deviceId}`;

  const refCallback = useCallback(
    (el: HTMLButtonElement | null) => {
      registry.register(portId, el);
    },
    [registry, portId],
  );

  return (
    <button
      ref={refCallback}
      type="button"
      aria-label={`${kind === "src" ? "Source" : "Sink"} port for device ${deviceId} on peer ${peerId}`}
      className={cn(
        "w-3 h-3 rounded-full border-2 border-line-2 bg-board cursor-crosshair",
        "transition-colors duration-100",
        "hover:border-gold focus-visible:border-gold",
        "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-gold/40",
      )}
      style={
        wired && color
          ? { backgroundColor: color, borderColor: color }
          : undefined
      }
      onClick={() => onActivate?.(portId, kind, peerId, deviceId)}
    />
  );
}
