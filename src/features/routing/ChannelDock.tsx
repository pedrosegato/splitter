import type { StreamSnapshot } from "@/bindings";
import { useUiStore } from "@/stores/ui";
import { Skeleton } from "@/components/ui/skeleton";
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
    <div className="flex flex-none items-stretch bg-elev-0 border-t border-line overflow-x-auto min-h-[96px]">
      {isLoading ? (
        <div className="flex items-center gap-3 px-4">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="w-[72px] h-[64px] bg-line-2 rounded-[2px]" />
          ))}
        </div>
      ) : (
        streams.map((stream) => (
          <ChannelStrip
            key={stream.id}
            sessionId={sessionId!}
            stream={stream}
            selected={selectedStreamId === stream.id}
          />
        ))
      )}
    </div>
  );
}
