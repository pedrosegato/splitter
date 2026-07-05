import { useEffect, useCallback } from "react";
import { useUiStore } from "@/stores/ui";
import { useDevices } from "@/hooks/useDevices";
import { useActiveSession } from "@/hooks/useActiveSession";
import { useIdentity } from "@/hooks/useIdentity";
import { useOpenStream, useRequestStream } from "@/hooks/useStreams";

export function useWiring() {
  const arm = useUiStore((s) => s.arm);
  const armSource = useUiStore((s) => s.armSource);
  const clearArm = useUiStore((s) => s.clearArm);
  const { data: devices } = useDevices();
  const { data: identity } = useIdentity();
  const openStream = useOpenStream();
  const requestStream = useRequestStream();
  const selfPeerId = identity?.peer_id;

  const { session, peerDevices } = useActiveSession();

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") clearArm();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [clearArm]);

  const onPortActivate = useCallback(
    (portId: string, kind: "src" | "sink", peerId: string, deviceId: string) => {
      void portId;

      if (!session) return;

      if (!arm) {
        armSource(peerId, deviceId, kind);
        return;
      }

      if (kind === arm.kind) {
        if (peerId === arm.peerId && deviceId === arm.deviceId) {
          clearArm();
        }
        return;
      }
      if (peerId === arm.peerId) return;

      const src =
        arm.kind === "src"
          ? { peer: arm.peerId, dev: arm.deviceId }
          : { peer: peerId, dev: deviceId };
      const sink =
        arm.kind === "sink"
          ? { peer: arm.peerId, dev: arm.deviceId }
          : { peer: peerId, dev: deviceId };

      if (src.peer === selfPeerId) {
        const armedDevice = devices?.find((d) => d.id === src.dev);
        openStream.mutate({
          sessionId: session.id,
          sourceDeviceId: src.dev,
          sourceIsSystem: armedDevice?.kind === "SystemAudio",
          sinkPeerId: sink.peer,
          sinkDeviceId: sink.dev,
          bitrate: undefined,
        });
      } else {
        const remoteDevice = peerDevices?.find((d) => d.id === src.dev);
        // Invalid targets are visually disabled so stray clicks are ignored here.
        requestStream.mutate({
          sessionId: session.id,
          source:
            remoteDevice?.kind === "SystemAudio"
              ? { type: "system", device_id: src.dev }
              : { type: "mic", device_id: src.dev },
          sinkDeviceId: sink.dev,
        });
      }

      clearArm();
    },
    [
      arm,
      session,
      devices,
      peerDevices,
      selfPeerId,
      armSource,
      clearArm,
      openStream,
      requestStream,
    ],
  );

  return { arm, onPortActivate };
}
