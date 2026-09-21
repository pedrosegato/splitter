import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowRight } from "lucide-react";
import { motion } from "motion/react";

import type { StreamSnapshot } from "@/bindings";
import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import { Toggle } from "@/components/ui/toggle";
import { useCloseStream, useStreamControl } from "@/hooks/useStreams";
import { deviceLabel } from "@/lib/deviceName";
import { variants } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { streamColor } from "./useWireGeometry";

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
        "border-line relative flex w-[232px] flex-none cursor-default flex-col justify-center border-r px-3 py-2.5 transition-colors",
        selected ? "bg-elev-2" : "bg-elev-0 hover:bg-elev-2",
      )}
      style={selected ? { boxShadow: "inset 3px 0 0 var(--color-gold)" } : undefined}
    >
      <div className="mb-2 flex items-center gap-2">
        <span className="h-[18px] w-2 flex-none rounded-[1px]" style={{ background: color }} />
        <span className="text-ink flex min-w-0 flex-1 items-center gap-1 text-[11px] font-medium">
          <span className="min-w-0 truncate">{deviceLabel(stream.source_device)}</span>
          <ArrowRight className="text-ink-3 size-3 flex-none" strokeWidth={2.5} />
          <span className="min-w-0 truncate">{deviceLabel(stream.sink_device)}</span>
        </span>
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={handleClose}
          className="text-ink-3 hover:text-gold flex-none text-[10px] leading-none hover:bg-transparent"
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
            "size-6 min-w-6 flex-none border p-0 text-[10px] font-medium",
            muted
              ? "bg-gold border-gold hover:bg-gold text-[#161618]"
              : "border-line-2 text-ink-2 hover:border-gold hover:text-gold bg-transparent hover:bg-transparent",
          )}
          aria-label={muted ? "desmutar" : "mutar"}
        >
          M
        </Toggle>

        <div className="flex-1" style={{ "--primary": color } as React.CSSProperties}>
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
