import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { usePendingPeers } from "@/hooks/usePeers";
import { useAcceptPending, useRejectPending } from "@/hooks/useConnection";
import { Check, X } from "lucide-react";
import { motion } from "motion/react";
import { variants } from "@/lib/motion";

export function IncomingRequestDialog() {
  const pendingPeers = usePendingPeers();
  const accept = useAcceptPending();
  const reject = useRejectPending();

  const peer = (pendingPeers.data ?? [])[0];
  const busy = accept.isPending || reject.isPending;

  return (
    <Dialog open={!!peer}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[320px] max-w-[320px] bg-surface border-line gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-elev-1 border-b border-line rounded-t-lg">
          <DialogTitle className="text-[9.5px] tracking-[0.5px] text-ink-3 font-semibold uppercase">
            Pedido de conexão
          </DialogTitle>
        </DialogHeader>

        {peer && (
          <motion.div
            variants={variants.listStagger}
            initial="hidden"
            animate="show"
            className="px-[15px] py-[16px]"
          >
            <motion.div variants={variants.listItem} className="flex items-center gap-[10px]">
              <span className="w-[8px] h-[8px] rounded-full bg-gold shrink-0" />
              <div className="min-w-0">
                <p className="truncate text-[13px] text-ink font-semibold">{peer.peer_name}</p>
                <p className="text-[10px] text-ink-3 tabular-nums">{peer.addr}</p>
              </div>
            </motion.div>

            <div className="mt-[16px] flex items-center justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => reject.mutate({ index: 0 })}
                className="text-[12px]"
              >
                <X size={14} />
                Recusar
              </Button>
              <Button
                size="sm"
                disabled={busy}
                onClick={() => accept.mutate({ index: 0 })}
                className="text-[12px] text-[#1c1c1f] bg-gold font-semibold hover:brightness-110"
              >
                <Check size={14} />
                Aceitar
              </Button>
            </div>
          </motion.div>
        )}
      </DialogContent>
    </Dialog>
  );
}
