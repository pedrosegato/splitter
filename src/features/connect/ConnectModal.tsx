import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { usePeers, usePendingPeers } from "@/hooks/usePeers";
import {
  useConnectPeer,
  useAcceptPending,
  useOpenSession,
} from "@/hooks/useConnection";
import type { DiscoveredPeer, PendingPeerDto } from "@/bindings";

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
};

function DiscoveredRow({
  peer,
  onSuccess,
}: {
  peer: DiscoveredPeer;
  onSuccess: () => void;
}) {
  const connectPeer = useConnectPeer();
  const openSession = useOpenSession();
  const isPending = connectPeer.isPending || openSession.isPending;

  function handlePairing() {
    connectPeer.mutate(
      { host: peer.host, port: peer.port, peerId: peer.peer_id },
      {
        onSuccess: () => {
          openSession.mutate(
            { remotePeerId: peer.peer_id },
            { onSuccess },
          );
        },
      },
    );
  }

  return (
    <div className="flex items-center gap-[11px] px-[11px] py-[10px] rounded-[2px] border border-transparent hover:bg-[#26262a] hover:border-line-2 cursor-default">
      <span className="w-[7px] h-[7px] rounded-full bg-green shrink-0" />
      <span className="flex-1 text-[12.5px]">
        {peer.peer_name}
        <small className="block text-ink-3 text-[10px]">
          {peer.host} · {peer.version}
        </small>
      </span>
      <button
        type="button"
        disabled={isPending}
        onClick={handlePairing}
        className="font-mono text-[10px] text-gold disabled:text-ink-3 disabled:cursor-not-allowed cursor-pointer"
      >
        parear
      </button>
    </div>
  );
}

function PendingRow({
  peer,
  index,
}: {
  peer: PendingPeerDto;
  index: number;
}) {
  const acceptPending = useAcceptPending();

  return (
    <div className="flex items-center gap-[11px] px-[11px] py-[10px] rounded-[2px] border border-transparent hover:bg-[#26262a] hover:border-line-2 cursor-default">
      <span className="w-[7px] h-[7px] rounded-full bg-gold shrink-0" />
      <span className="flex-1 text-[12.5px]">
        {peer.peer_name}
        <small className="block text-ink-3 text-[10px]">{peer.addr}</small>
      </span>
      <button
        type="button"
        disabled={acceptPending.isPending}
        onClick={() => acceptPending.mutate({ index })}
        className="font-mono text-[10px] text-gold disabled:text-ink-3 disabled:cursor-not-allowed cursor-pointer"
      >
        aceitar
      </button>
    </div>
  );
}

export function ConnectModal({ open, onOpenChange }: Props) {
  const peers = usePeers();
  const pendingPeers = usePendingPeers();

  const discovered = peers.data ?? [];
  const pending = pendingPeers.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[378px] max-w-[378px] bg-surface border-line rounded-[3px] gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-[#2a2a2d] border-b border-line rounded-t-[3px]">
          <DialogTitle className="font-mono text-[9.5px] tracking-[0.5px] text-ink-3 font-semibold uppercase">
            Máquinas na rede
          </DialogTitle>
        </DialogHeader>

        <div className="px-[7px] py-[7px]">
          {discovered.length === 0 ? (
            <p className="px-[11px] py-[10px] text-[12px] text-ink-3">
              nenhuma máquina na rede
            </p>
          ) : (
            discovered.map((peer) => (
              <DiscoveredRow
                key={peer.peer_id}
                peer={peer}
                onSuccess={() => onOpenChange(false)}
              />
            ))
          )}
        </div>

        {pending.length > 0 && (
          <>
            <div className="h-px bg-line mx-[11px]" />
            <div className="px-[7px] py-[7px]">
              {pending.map((peer, i) => (
                <PendingRow key={peer.peer_id} peer={peer} index={i} />
              ))}
            </div>
          </>
        )}

        <div className="h-px bg-line mx-[11px]" />

        <div className="flex items-center justify-between px-[13px] py-[9px] border-t border-line">
          <span className="font-mono text-[9.5px] text-ink-3">
            1ª conexão pede confiar no dispositivo
          </span>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            className="font-mono text-[11px] text-ink-2 bg-[#242426] border border-line-2 rounded-[2px] px-3 py-[5px] cursor-pointer hover:text-ink hover:border-line"
          >
            cancelar
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
