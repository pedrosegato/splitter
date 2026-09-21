import { useState } from "react";
import { Cable } from "lucide-react";
import { motion } from "motion/react";

import type { DiscoveredPeer } from "@/bindings";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { useConnectPeer, useOpenSession } from "@/hooks/useConnection";
import { usePeers } from "@/hooks/usePeers";
import { variants } from "@/lib/motion";

const DEFAULT_PORT = 7000;

type Props = {
  open: boolean;
  onOpenChange: (o: boolean) => void;
};

function DiscoveredRow({ peer, onSuccess }: { peer: DiscoveredPeer; onSuccess: () => void }) {
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
      className="group hover:bg-elev-2 hover:border-line-2 h-auto w-full justify-start gap-[11px] border border-transparent px-[11px] py-[10px] text-left disabled:cursor-default disabled:opacity-50"
    >
      <span className="bg-green h-[7px] w-[7px] shrink-0 rounded-full" />
      <span className="min-w-0 flex-1 truncate text-[12.5px]">
        {peer.peer_name}
        <small className="text-ink-3 block truncate text-[10px]">
          {peer.host} · {peer.version}
        </small>
      </span>
      <Cable size={15} className="text-ink-3 group-hover:text-gold shrink-0" />
    </Button>
  );
}

function ManualConnectRow({ onSuccess }: { onSuccess: () => void }) {
  const connectPeer = useConnectPeer();
  const openSession = useOpenSession();
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const isPending = connectPeer.isPending || openSession.isPending;

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = host.trim();
    if (!trimmed || isPending) return;
    const parsedPort = Number(port) || DEFAULT_PORT;
    connectPeer.mutate(
      { host: trimmed, port: parsedPort, peerId: null },
      {
        onSuccess: (peerId) => {
          if (!peerId) return;
          openSession.mutate({ remotePeerId: peerId }, { onSuccess });
        },
      },
    );
  }

  return (
    <form
      onSubmit={handleSubmit}
      className="border-line flex items-center gap-[7px] border-t px-[13px] py-[10px]"
    >
      <Input
        value={host}
        onChange={(e) => setHost(e.target.value)}
        placeholder="192.168.1.5"
        aria-label="endereço do peer"
        className="h-8 flex-1 text-[12px]"
      />
      <Input
        value={port}
        onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
        placeholder={String(DEFAULT_PORT)}
        inputMode="numeric"
        aria-label="porta"
        className="h-8 w-16 text-[12px]"
      />
      <Button
        type="submit"
        size="sm"
        variant="secondary"
        disabled={!host.trim() || isPending}
        className="h-8 gap-[6px] text-[11px]"
      >
        <Cable size={14} />
        conectar
      </Button>
    </form>
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
        className="bg-surface border-line w-[378px] max-w-[378px] gap-0 p-0"
      >
        <DialogHeader className="bg-elev-1 border-line rounded-t-lg border-b px-[15px] py-3">
          <DialogTitle className="text-ink-3 text-[11px] font-medium">Máquinas na rede</DialogTitle>
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
              <DiscoveredRow key={peer.peer_id} peer={peer} onSuccess={() => onOpenChange(false)} />
            ))
          )}
        </motion.div>

        <ManualConnectRow onSuccess={() => onOpenChange(false)} />

        <div className="border-line flex items-center justify-end border-t px-[13px] py-[9px]">
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
