import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
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
        className="w-[320px] max-w-[320px] bg-surface border-line rounded-[3px] gap-0 p-0"
      >
        <DialogHeader className="px-[15px] py-3 bg-elev-1 border-b border-line rounded-t-[3px]">
          <DialogTitle className="font-mono text-[9.5px] tracking-[0.5px] text-ink-3 font-semibold uppercase">
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
              <button
                type="button"
                disabled={busy}
                onClick={() => reject.mutate({ index: 0 })}
                className="flex items-center gap-1.5 text-[12px] text-ink-2 bg-elev-2 border border-line-2 rounded-[2px] px-3 py-[6px] cursor-pointer hover:text-ink hover:border-line disabled:opacity-50"
              >
                <X size={14} />
                Recusar
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => accept.mutate({ index: 0 })}
                className="flex items-center gap-1.5 text-[12px] text-[#1c1c1f] bg-gold border border-gold rounded-[2px] px-3 py-[6px] cursor-pointer font-semibold hover:brightness-110 disabled:opacity-50"
              >
                <Check size={14} />
                Aceitar
              </button>
            </div>
          </motion.div>
        )}
      </DialogContent>
    </Dialog>
  );
}
