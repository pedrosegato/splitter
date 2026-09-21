import { Check, X } from "lucide-react";
import { motion } from "motion/react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useAcceptPending, useRejectPending } from "@/hooks/useConnection";
import { usePendingPeers } from "@/hooks/usePeers";
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
        className="bg-surface border-line w-[320px] max-w-[320px] gap-0 p-0"
      >
        <DialogHeader className="bg-elev-1 border-line rounded-t-lg border-b px-[15px] py-3">
          <DialogTitle className="text-ink-3 text-[11px] font-medium">
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
              <span className="bg-gold h-[8px] w-[8px] shrink-0 rounded-full" />
              <div className="min-w-0">
                <p className="text-ink truncate text-[13px] font-semibold">{peer.peer_name}</p>
                <p className="text-ink-3 text-[10px] tabular-nums">{peer.addr}</p>
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
                className="bg-gold text-[12px] font-semibold text-[#1c1c1f] hover:brightness-110"
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
