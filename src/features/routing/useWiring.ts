import { useState, useEffect, useCallback } from "react";
import { useUiStore } from "@/stores/ui";
import { useSnapshot } from "@/hooks/useSnapshot";
import { useDevices, usePeerDevices } from "@/hooks/useDevices";
import { useIdentity } from "@/hooks/useIdentity";
import { useOpenStream, useRequestStream } from "@/hooks/useStreams";

export function useWiring() {
  const arm = useUiStore((s) => s.arm);
  const armSource = useUiStore((s) => s.armSource);
  const clearArm = useUiStore((s) => s.clearArm);
  const { data: snap } = useSnapshot();
  const { data: devices } = useDevices();
  const { data: identity } = useIdentity();
  const openStream = useOpenStream();
  const requestStream = useRequestStream();
  const [hint, setHint] = useState<string | null>(null);
  const selfPeerId = identity?.peer_id;

  const session = (snap ?? []).find((s) => s.state === "active") ?? null;
  const { data: peerDevices } = usePeerDevices(session?.remote_peer_id);

  useEffect(() => {
    if (!hint) return;
    const timer = setTimeout(() => setHint(null), 2200);
    return () => clearTimeout(timer);
  }, [hint]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        clearArm();
        setHint(null);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [clearArm]);

  const onPortActivate = useCallback(
    (portId: string, kind: "src" | "sink", peerId: string, deviceId: string) => {
      void portId;

      if (!session) {
        setHint("conecte uma máquina primeiro");
        return;
      }

      if (!arm) {
        if (kind !== "src") {
          setHint("comece por uma fonte");
          return;
        }
        armSource(peerId, deviceId);
        setHint("agora clique num destino");
        return;
      }

      if (kind !== "sink") {
        setHint("termine num destino");
        return;
      }

      if (peerId === arm.peerId) {
        setHint("não pode rotear pra mesma máquina");
        clearArm();
        return;
      }

      const sourceIsSelf = arm.peerId === selfPeerId;

      if (sourceIsSelf) {
        const armedDevice = devices?.find((d) => d.id === arm.deviceId);
        openStream.mutate({
          sessionId: session.id,
          sourceDeviceId: arm.deviceId,
          sourceIsSystem: armedDevice?.kind === "SystemAudio",
          sinkPeerId: peerId,
          sinkDeviceId: deviceId,
          bitrate: undefined,
        });
      } else {
        const remoteDevice = peerDevices?.find((d) => d.id === arm.deviceId);
        requestStream.mutate({
          sessionId: session.id,
          sourceDeviceId: arm.deviceId,
          sourceIsSystem: remoteDevice?.kind === "SystemAudio",
          sinkDeviceId: deviceId,
        });
      }

      clearArm();
      setHint(null);
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

  return { arm, hint, onPortActivate };
}
