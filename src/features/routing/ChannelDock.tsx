import type { StreamSnapshot } from "@/bindings";
import { useUiStore } from "@/stores/ui";
import { ChannelStrip } from "./ChannelStrip";

type Props = {
  sessionId: string | null;
  streams: StreamSnapshot[];
};

export function ChannelDock({ sessionId, streams }: Props) {
  const selectedStreamId = useUiStore((s) => s.selectedStreamId);

  return (
    <div className="flex flex-none items-stretch bg-[#161618] border-t border-line overflow-x-auto min-h-[96px]">
      {streams.length === 0 || sessionId === null ? (
        <div className="flex flex-1 items-center justify-center text-[11.5px] text-ink-3">
          sem streams
        </div>
      ) : (
        streams.map((stream) => (
          <ChannelStrip
            key={stream.id}
            sessionId={sessionId}
            stream={stream}
            selected={selectedStreamId === stream.id}
          />
        ))
      )}
    </div>
  );
}
