import { Check, X } from "lucide-react";
import { motion } from "motion/react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  useAcceptStreamRequest,
  useRejectStreamRequest,
  useStreamRequests,
} from "@/hooks/useStreamRequests";
import { variants } from "@/lib/motion";

export function StreamRequestDialog() {
  const requests = useStreamRequests();
  const accept = useAcceptStreamRequest();
  const reject = useRejectStreamRequest();

  const request = (requests.data ?? [])[0];
  const busy = accept.isPending || reject.isPending;

  return (
    <Dialog open={!!request}>
      <DialogContent
        showCloseButton={false}
        aria-describedby={undefined}
        className="bg-surface border-line w-[320px] max-w-[320px] gap-0 p-0"
      >
        <DialogHeader className="bg-elev-1 border-line rounded-t-lg border-b px-[15px] py-3">
          <DialogTitle className="text-ink-3 text-[11px] font-medium">Pedido de áudio</DialogTitle>
        </DialogHeader>

        {request && (
          <motion.div
            variants={variants.listStagger}
            initial="hidden"
            animate="show"
            className="px-[15px] py-[16px]"
          >
            <motion.div variants={variants.listItem} className="flex items-center gap-[10px]">
              <span className="bg-gold h-[8px] w-[8px] shrink-0 rounded-full" />
              <div className="min-w-0">
                <p className="text-ink truncate text-[13px] font-semibold">{request.peer_name}</p>
                <p className="text-ink-3 text-[10px]">
                  {request.source_is_system
                    ? `quer receber o áudio do sistema (${request.source_device_name})`
                    : `quer receber seu microfone: ${request.source_device_name}`}
                </p>
              </div>
            </motion.div>

            <div className="mt-[16px] flex items-center justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => reject.mutate({ requestId: request.id })}
                className="text-[12px]"
              >
                <X size={14} />
                Recusar
              </Button>
              <Button
                size="sm"
                disabled={busy}
                onClick={() => accept.mutate({ requestId: request.id })}
                className="bg-gold text-[12px] font-semibold text-[#1c1c1f] hover:brightness-110"
              >
                <Check size={14} />
                Permitir
              </Button>
            </div>
          </motion.div>
        )}
      </DialogContent>
    </Dialog>
  );
}
