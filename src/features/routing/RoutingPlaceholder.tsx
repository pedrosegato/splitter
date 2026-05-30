import { useDevices } from "@/hooks/useDevices";
import { usePeers } from "@/hooks/usePeers";

export function RoutingPlaceholder() {
  const { data: devices } = useDevices();
  const { data: peers } = usePeers();

  const deviceCount = devices?.length ?? 0;
  const peerList = peers ?? [];

  return (
    <div className="p-4 flex flex-col gap-3">
      <span className="text-ink-2 text-xs">
        {deviceCount} dispositivo{deviceCount !== 1 ? "s" : ""} local{deviceCount !== 1 ? "is" : ""}
      </span>
      <div className="flex flex-col gap-1">
        {peerList.length === 0 ? (
          <span className="text-ink-3 text-xs">nenhum peer na rede</span>
        ) : (
          peerList.map((p) => (
            <span key={p.peer_id} className="text-ink text-xs">
              {p.peer_name}
            </span>
          ))
        )}
      </div>
    </div>
  );
}
