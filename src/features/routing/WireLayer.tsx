import { useLayoutEffect, useState, useCallback, useRef } from "react";
import {
  AnimatePresence,
  motion,
  useTransform,
  useMotionValue,
  useReducedMotion,
} from "motion/react";
import type { StreamSnapshot } from "@/bindings";
import { usePortRegistry } from "./usePortRegistry";
import { cable, sagFor, streamColor, type Pt } from "./useWireGeometry";
import { useThemeStore } from "@/stores/theme";
import { Wire } from "./Wire";
import type { DragState } from "./useDragConnect";

type WireLayerProps = {
  boardRef: React.RefObject<HTMLDivElement | null>;
  streams: StreamSnapshot[];
  selectedId: number | null;
  onSelect: (streamId: number | null) => void;
  drag?: DragState;
};

type ComputedWire = {
  id: number;
  a: Pt;
  b: Pt;
  color: string;
  muted: boolean;
  volume: number;
};

export function WireLayer({ boardRef, streams, selectedId, onSelect, drag }: WireLayerProps) {
  const registry = usePortRegistry();
  const theme = useThemeStore((s) => s.theme);
  const reducedMotion = useReducedMotion();
  const [wires, setWires] = useState<ComputedWire[]>([]);

  const measure = useCallback(() => {
    const board = boardRef.current;
    if (!board) return;
    const boardRect = board.getBoundingClientRect();

    const computed: ComputedWire[] = [];
    for (const stream of streams) {
      const srcId = `${stream.source_peer}:src:${stream.source_device}`;
      const sinkId = `${stream.sink_peer}:sink:${stream.sink_device}`;
      const srcEl = registry.get(srcId);
      const sinkEl = registry.get(sinkId);
      if (!srcEl || !sinkEl) continue;

      const sr = srcEl.getBoundingClientRect();
      const kr = sinkEl.getBoundingClientRect();
      const a = {
        x: sr.left + sr.width / 2 - boardRect.left,
        y: sr.top + sr.height / 2 - boardRect.top,
      };
      const b = {
        x: kr.left + kr.width / 2 - boardRect.left,
        y: kr.top + kr.height / 2 - boardRect.top,
      };

      computed.push({
        id: stream.id,
        a,
        b,
        color: streamColor(stream.id),
        muted: stream.state === "paused" || stream.muted,
        volume: stream.volume,
      });
    }

    setWires(computed);
  }, [boardRef, streams, registry]);

  const measureRef = useRef(measure);
  measureRef.current = measure;

  useLayoutEffect(() => {
    measure();
  }, [measure]);

  useLayoutEffect(() => {
    const board = boardRef.current;
    if (!board) return;

    const ro = new ResizeObserver(() => measureRef.current());
    ro.observe(board);

    const onWindowResize = () => measureRef.current();
    window.addEventListener("resize", onWindowResize);

    return () => {
      ro.disconnect();
      window.removeEventListener("resize", onWindowResize);
    };
  }, [boardRef]);

  useLayoutEffect(() => {
    const id = requestAnimationFrame(() => requestAnimationFrame(measure));
    return () => cancelAnimationFrame(id);
  }, [theme, measure]);

  const board = boardRef.current;

  let dragOrigin: Pt | null = null;
  if (drag?.active && drag.from && board) {
    const fromPortId = `${drag.from.peerId}:${drag.from.kind}:${drag.from.deviceId}`;
    const el = registry.get(fromPortId);
    if (el) {
      const br = board.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      dragOrigin = {
        x: r.left + r.width / 2 - br.left,
        y: r.top + r.height / 2 - br.top,
      };
    }
  }

  const fallbackX = useMotionValue(0);
  const fallbackY = useMotionValue(0);
  const liveX = drag?.x ?? fallbackX;
  const liveY = drag?.y ?? fallbackY;
  const liveDragPath = useTransform([liveX, liveY], (latest) => {
    const [x, y] = latest as number[];
    if (!dragOrigin) return "";
    const to = { x, y };
    return cable(dragOrigin, to, sagFor(dragOrigin, to));
  });

  const showLiveDrag = Boolean(drag?.active && drag.from && dragOrigin);
  const hasSelection = selectedId !== null;

  return (
    <svg className="absolute inset-0 w-full h-full pointer-events-none z-[1]">
      {showLiveDrag && (
        <motion.path
          d={liveDragPath}
          fill="none"
          stroke="var(--color-gold)"
          strokeWidth="2.5"
          strokeLinecap="round"
          initial={{ opacity: 0 }}
          animate={{ opacity: 0.85 }}
          style={{ pointerEvents: "none" }}
        />
      )}
      <AnimatePresence>
        {wires.map((wire) => (
          <Wire
            key={wire.id}
            id={wire.id}
            a={wire.a}
            b={wire.b}
            color={wire.color}
            muted={wire.muted}
            volume={wire.volume}
            selected={selectedId === wire.id}
            hasSelection={hasSelection}
            reducedMotion={!!reducedMotion}
            onSelect={onSelect}
          />
        ))}
      </AnimatePresence>
    </svg>
  );
}
