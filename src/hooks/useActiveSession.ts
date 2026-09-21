import { useMemo } from "react";

import type { DeviceDescriptor, SessionSnapshot, StreamSnapshot } from "@/bindings";

import { usePeerDevices } from "./useDevices";
import { useSnapshot } from "./useSnapshot";

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
