import { useCallback, useEffect, useRef, useState } from "react";
import { motion } from "motion/react";
import { ArrowRight } from "lucide-react";
import type { StreamSnapshot } from "@/bindings";
import { useCloseStream, useStreamControl } from "@/hooks/useStreams";
import { useUiStore } from "@/stores/ui";
import { Slider } from "@/components/ui/slider";
import { Button } from "@/components/ui/button";
import { Toggle } from "@/components/ui/toggle";
import { streamColor } from "./useWireGeometry";
import { deviceLabel } from "@/lib/deviceName";
import { variants } from "@/lib/motion";
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

  const muted = stream.muted;

  const color = streamColor(stream.id);

  const authoritativeVolume = Math.round(stream.volume * 100);
  const [dragVolume, setDragVolume] = useState<number | null>(null);
  const draggingRef = useRef(false);
  const lastSentRef = useRef(0);

  useEffect(() => {
    if (!draggingRef.current) setDragVolume(null);
  }, [authoritativeVolume]);

  const displayVolume = dragVolume ?? authoritativeVolume;

  const sendVolume = useCallback(
    (value: number) => {
      streamControl.mutate({
        sessionId,
        streamId: stream.id,
        action: { type: "set_volume", volume: value / 100 },
      });
    },
    [sessionId, stream.id, streamControl],
  );

  const handleMute = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      streamControl.mutate({
        sessionId,
        streamId: stream.id,
        action: { type: "set_muted", muted: !stream.muted },
      });
    },
    [sessionId, stream.id, stream.muted, streamControl],
  );

  const handleVolumeChange = useCallback(
    (values: number[]) => {
      const value = values[0];
      draggingRef.current = true;
      setDragVolume(value);
      const now = Date.now();
      if (now - lastSentRef.current >= 80) {
        lastSentRef.current = now;
        sendVolume(value);
      }
    },
    [sendVolume],
  );

  const handleVolumeCommit = useCallback(
    (values: number[]) => {
      const value = values[0];
      draggingRef.current = false;
      lastSentRef.current = Date.now();
      sendVolume(value);
    },
    [sendVolume],
  );

  const handleClose = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      closeStream.mutate({ sessionId, streamId: stream.id });
    },
    [sessionId, stream.id, closeStream],
  );

  return (
    <motion.div
      layout
      variants={variants.listItem}
      initial="hidden"
      animate="show"
      exit={{ opacity: 0, y: -6 }}
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
        <span className="flex flex-1 min-w-0 items-center gap-1 text-[11px] font-medium text-ink">
          <span className="min-w-0 truncate">{deviceLabel(stream.source_device)}</span>
          <ArrowRight className="size-3 flex-none text-ink-3" strokeWidth={2.5} />
          <span className="min-w-0 truncate">{deviceLabel(stream.sink_device)}</span>
        </span>
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={handleClose}
          className="flex-none text-[10px] text-ink-3 hover:bg-transparent hover:text-gold leading-none"
          aria-label="fechar stream"
        >
          ✕
        </Button>
      </div>

      <div className="flex items-center gap-2.5">
        <Toggle
          pressed={muted}
          onClick={handleMute}
          className={cn(
            "flex-none size-6 min-w-6 p-0 border text-[10px] font-medium",
            muted
              ? "bg-gold border-gold text-[#161618] hover:bg-gold"
              : "bg-transparent border-line-2 text-ink-2 hover:bg-transparent hover:border-gold hover:text-gold",
          )}
          aria-label={muted ? "desmutar" : "mutar"}
        >
          M
        </Toggle>

        <div
          className="flex-1"
          style={{ "--primary": color } as React.CSSProperties}
        >
          <Slider
            value={[displayVolume]}
            min={0}
            max={100}
            onValueChange={handleVolumeChange}
            onValueCommit={handleVolumeCommit}
            aria-label="volume"
          />
        </div>
      </div>
    </motion.div>
  );
}
