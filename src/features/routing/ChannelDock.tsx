import { AnimatePresence } from "motion/react";

import type { StreamSnapshot } from "@/bindings";
import { Skeleton } from "@/components/ui/skeleton";
import { useUiStore } from "@/stores/ui";

import { ChannelStrip } from "./ChannelStrip";

type Props = {
  sessionId: string | null;
  streams: StreamSnapshot[];
  isLoading?: boolean;
};

export function ChannelDock({ sessionId, streams, isLoading }: Props) {
  const selectedStreamId = useUiStore((s) => s.selectedStreamId);

  if (!isLoading && (streams.length === 0 || sessionId === null)) {
    return null;
  }

  return (
    <div className="bg-elev-0 border-line flex min-h-[96px] flex-none items-stretch overflow-x-auto border-t">
      {isLoading ? (
        <div className="flex items-center gap-3 px-4">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="bg-line-2 h-[64px] w-[72px]" />
          ))}
        </div>
      ) : (
        <AnimatePresence initial={false}>
          {streams.map((stream) => (
            <ChannelStrip
              key={stream.id}
              sessionId={sessionId!}
              stream={stream}
              selected={selectedStreamId === stream.id}
            />
          ))}
        </AnimatePresence>
      )}
    </div>
  );
}
