import { useCallback, useState } from "react";
import type { StreamSnapshot } from "@/bindings";
import { useCloseStream, useStreamControl } from "@/hooks/useStreams";
import { useUiStore } from "@/stores/ui";
import { Slider } from "@/components/ui/slider";
import { streamColor } from "./useWireGeometry";
import { cn } from "@/lib/utils";

type Props = {
  sessionId: string;
  stream: StreamSnapshot;
  selected: boolean;
};

export function ChannelStrip({ sessionId, stream, selected }: Props) {
  const selectStream = useUiStore((s) => s.selectStream);
  const streamControl = useStreamControl();
  const closeStream = useCloseStream();

  const [muted, setMuted] = useState(false);

  const color = streamColor(stream.id);
  const initialVolume = Math.round(stream.volume * 100);

  const handleMute = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      const next = !muted;
      setMuted(next);
      streamControl.mutate({
        sessionId,
        streamId: stream.id,
        action: { type: "set_muted", muted: next },
      });
    },
    [muted, sessionId, stream.id, streamControl],
  );

  const handleVolumeChange = useCallback(
    (values: number[]) => {
      streamControl.mutate({
        sessionId,
        streamId: stream.id,
        action: { type: "set_volume", volume: values[0] / 100 },
      });
    },
    [sessionId, stream.id, streamControl],
  );

  const handleClose = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      closeStream.mutate({ sessionId, streamId: stream.id });
    },
    [sessionId, stream.id, closeStream],
  );

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => selectStream(stream.id)}
      onKeyDown={(e) => e.key === "Enter" && selectStream(stream.id)}
      className={cn(
        "relative flex w-[232px] flex-none flex-col justify-center border-r border-line px-3 py-2.5 cursor-default transition-colors",
        selected ? "bg-elev-2" : "bg-elev-0 hover:bg-elev-2",
      )}
      style={
        selected
          ? { boxShadow: "inset 3px 0 0 var(--color-gold)" }
          : undefined
      }
    >
      <div className="flex items-center gap-2 mb-2">
        <span
          className="w-2 h-[18px] rounded-[1px] flex-none"
          style={{ background: color }}
        />
        <span className="text-[11px] font-medium overflow-hidden text-ellipsis whitespace-nowrap flex-1 text-ink">
          {stream.source_device} → {stream.sink_device}
        </span>
        <button
          type="button"
          onClick={handleClose}
          className="flex-none text-[10px] text-ink-3 hover:text-gold leading-none px-0.5"
          aria-label="fechar stream"
        >
          ✕
        </button>
      </div>

      <div className="flex items-center gap-2.5">
        <button
          type="button"
          onClick={handleMute}
          className={cn(
            "flex-none w-6 h-6 rounded-[2px] border text-[10px] font-mono font-medium flex items-center justify-center",
            muted
              ? "bg-gold border-gold text-[#161618]"
              : "bg-transparent border-line-2 text-ink-2 hover:border-gold hover:text-gold",
          )}
          aria-label={muted ? "desmutar" : "mutar"}
        >
          M
        </button>

        <div
          className="flex-1"
          style={{ "--primary": color } as React.CSSProperties}
        >
          <Slider
            defaultValue={[initialVolume]}
            min={0}
            max={100}
            onValueChange={handleVolumeChange}
            aria-label="volume"
          />
        </div>
      </div>
    </div>
  );
}
