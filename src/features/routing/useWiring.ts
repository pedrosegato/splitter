import { useState, useEffect, useCallback } from "react";
import { useUiStore } from "@/stores/ui";
import { useSnapshot } from "@/hooks/useSnapshot";
import { useDevices } from "@/hooks/useDevices";
import { useOpenStream } from "@/hooks/useStreams";

export function useWiring() {
  const arm = useUiStore((s) => s.arm);
  const armSource = useUiStore((s) => s.armSource);
  const clearArm = useUiStore((s) => s.clearArm);
  const { data: snap } = useSnapshot();
  const { data: devices } = useDevices();
  const openStream = useOpenStream();
  const [hint, setHint] = useState<string | null>(null);

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

      const sessions = snap ?? [];

      if (sessions.length === 0) {
        setHint("conecte uma máquina primeiro");
        return;
      }

      if (!arm) {
        if (kind !== "src") {
          setHint("comece por uma fonte (direita)");
          return;
        }
        armSource(peerId, deviceId);
        setHint("clique num destino do outro PC");
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

      const session = sessions[0];
      const armedDevice = devices?.find((d) => d.id === arm.deviceId);
      const sourceIsSystem = armedDevice?.kind === "SystemAudio";

      openStream.mutate({
        sessionId: session.id,
        sourceDeviceId: arm.deviceId,
        sourceIsSystem,
        sinkPeerId: peerId,
        sinkDeviceId: deviceId,
        bitrate: undefined,
      });
      clearArm();
      setHint(null);
    },
    [arm, snap, devices, armSource, clearArm, openStream],
  );

  return { arm, hint, onPortActivate };
}
