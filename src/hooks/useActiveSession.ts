import { useMemo } from "react";
import type { SessionSnapshot, StreamSnapshot, DeviceDescriptor } from "@/bindings";
import { useSnapshot } from "./useSnapshot";
import { usePeerDevices } from "./useDevices";

type ActiveSession = {
  session: SessionSnapshot | null;
  streams: StreamSnapshot[];
  remotePeerId: string | undefined;
  peerDevices: DeviceDescriptor[] | undefined;
  isLoading: boolean;
};

export function useActiveSession(): ActiveSession {
  const { data: snapshots, isLoading } = useSnapshot();
  const session = snapshots?.find((s) => s.state === "active") ?? null;
  const remotePeerId = session?.remote_peer_id;
  const { data: peerDevices } = usePeerDevices(remotePeerId);
  const streams = useMemo(() => session?.streams ?? [], [session]);
  return { session, streams, remotePeerId, peerDevices, isLoading };
}
