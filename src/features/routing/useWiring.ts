import { useCallback } from "react";
import { useDevices } from "@/hooks/useDevices";
import { useActiveSession } from "@/hooks/useActiveSession";
import { useIdentity } from "@/hooks/useIdentity";
import { useOpenStream, useRequestStream } from "@/hooks/useStreams";
import { resolveConnection, type PortRef, type Connection } from "./resolveConnection";

export function useWiring() {
  const { data: devices } = useDevices();
  const { data: identity } = useIdentity();
  const openStream = useOpenStream();
  const requestStream = useRequestStream();
  const selfPeerId = identity?.peer_id;

  const { session, peerDevices } = useActiveSession();

  const runConnection = useCallback(
    (conn: NonNullable<Connection>) => {
      if (!session) return;
      const { src, sink } = conn;

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
        requestStream.mutate({
          sessionId: session.id,
          source:
            remoteDevice?.kind === "SystemAudio"
              ? { type: "system", device_id: src.dev }
              : { type: "mic", device_id: src.dev },
          sinkDeviceId: sink.dev,
        });
      }
    },
    [session, devices, peerDevices, selfPeerId, openStream, requestStream],
  );

  const onPortConnect = useCallback(
    (a: PortRef, b: PortRef) => {
      if (!session) return;
      const conn = resolveConnection(a, b);
      if (conn) runConnection(conn);
    },
    [session, runConnection],
  );

  return { onPortConnect };
}
