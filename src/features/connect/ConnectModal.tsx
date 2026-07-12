import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Button } from "@/components/ui/button";
import { usePeers } from "@/hooks/usePeers";
import { useConnectPeer, useOpenSession } from "@/hooks/useConnection";
import type { DiscoveredPeer } from "@/bindings";
import { Cable } from "lucide-react";
import { motion } from "motion/react";
import { variants } from "@/lib/motion";

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
    if (isPending) return;
    connectPeer.mutate(
      { host: peer.host, port: peer.port, peerId: peer.peer_id },
      {
        onSuccess: () => {
          openSession.mutate({ remotePeerId: peer.peer_id }, { onSuccess });
        },
      },
    );
  }

  return (
    <Button
      variant="ghost"
      onClick={handlePairing}
      disabled={isPending}
      className="group w-full h-auto justify-start gap-[11px] px-[11px] py-[10px] border border-transparent hover:bg-elev-2 hover:border-line-2 text-left disabled:opacity-50 disabled:cursor-default"
    >
      <span className="w-[7px] h-[7px] rounded-full bg-green shrink-0" />
      <span className="flex-1 min-w-0 text-[12.5px] truncate">
        {peer.peer_name}
        <small className="block text-ink-3 text-[10px] truncate">
          {peer.host} · {peer.version}
        </small>
      </span>
      <Cable size={15} className="shrink-0 text-ink-3 group-hover:text-gold" />
    </Button>
  );
}

export function ConnectModal({ open, onOpenChange }: Props) {
  const peers = usePeers();
  const discovered = peers.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[378px] max-w-[378px] bg-surface border-line gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-elev-1 border-b border-line rounded-t-lg">
          <DialogTitle className="text-[11px] text-ink-3 font-medium">
            Máquinas na rede
          </DialogTitle>
        </DialogHeader>

        <motion.div
          variants={variants.listStagger}
          initial="hidden"
          animate="show"
          className="px-[7px] py-[7px]"
        >
          {discovered.length === 0 ? (
            <Empty>
              <EmptyHeader>
                <EmptyTitle>nenhuma máquina na rede</EmptyTitle>
              </EmptyHeader>
            </Empty>
          ) : (
            discovered.map((peer) => (
              <DiscoveredRow
                key={peer.peer_id}
                peer={peer}
                onSuccess={() => onOpenChange(false)}
              />
            ))
          )}
        </motion.div>

        <div className="flex items-center justify-end px-[13px] py-[9px] border-t border-line">
          <Button
            variant="secondary"
            size="sm"
            onClick={() => onOpenChange(false)}
            className="text-[11px]"
          >
            cancelar
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
