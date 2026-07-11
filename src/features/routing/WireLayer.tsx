import { useLayoutEffect, useState, useRef, useCallback, useEffect } from "react";
import { motion, useTransform, useMotionValue, useReducedMotion } from "motion/react";
import type { StreamSnapshot } from "@/bindings";
import { usePortRegistry } from "./usePortRegistry";
import { curve, streamColor, type Pt } from "./useWireGeometry";
import { useUiStore } from "@/stores/ui";
import { useThemeStore } from "@/stores/theme";
import { springs } from "@/lib/motion";
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
  d: string;
  color: string;
  muted: boolean;
  volume: number;
  sink: Pt;
};

export function WireLayer({ boardRef, streams, selectedId, onSelect, drag }: WireLayerProps) {
  const registry = usePortRegistry();
  const arm = useUiStore((s) => s.arm);
  const theme = useThemeStore((s) => s.theme);
  const reducedMotion = useReducedMotion();
  const [wires, setWires] = useState<ComputedWire[]>([]);
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null);
  const [pulseIds, setPulseIds] = useState<number[]>([]);
  const seenIdsRef = useRef<Set<number> | null>(null);
  const tickRef = useRef(0);

  const measure = useCallback(() => {
    const board = boardRef.current;
    if (!board) return;
    const boardRect = board.getBoundingClientRect();
    const centerX = board.clientWidth / 2;

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
        d: curve(a, b, centerX),
        color: streamColor(stream.id),
        muted: stream.state === "paused" || stream.muted,
        volume: stream.volume,
        sink: b,
      });
    }

    setWires(computed);
  }, [boardRef, streams, registry]);

  useLayoutEffect(() => {
    measure();
  }, [measure]);

  useLayoutEffect(() => {
    const board = boardRef.current;
    if (!board) return;

    const ro = new ResizeObserver(() => measure());
    ro.observe(board);

    const onWindowResize = () => measure();
    window.addEventListener("resize", onWindowResize);

    return () => {
      ro.disconnect();
      window.removeEventListener("resize", onWindowResize);
    };
  }, [boardRef, measure]);

  useLayoutEffect(() => {
    const id = requestAnimationFrame(() => requestAnimationFrame(measure));
    return () => cancelAnimationFrame(id);
  }, [theme, measure]);

  useLayoutEffect(() => {
    const board = boardRef.current;
    if (!board || !arm) {
      setCursor(null);
      return;
    }
    const onMove = (e: PointerEvent) => {
      const br = board.getBoundingClientRect();
      setCursor({ x: e.clientX - br.left, y: e.clientY - br.top });
    };
    board.addEventListener("pointermove", onMove);
    return () => board.removeEventListener("pointermove", onMove);
  }, [boardRef, arm]);

  useEffect(() => {
    const currentIds = wires.map((wire) => wire.id);
    if (seenIdsRef.current === null) {
      seenIdsRef.current = new Set(currentIds);
      return;
    }
    const fresh = currentIds.filter((id) => !seenIdsRef.current!.has(id));
    for (const id of currentIds) seenIdsRef.current.add(id);
    if (fresh.length > 0) {
      setPulseIds((prev) => [...prev, ...fresh]);
    }
  }, [wires]);

  let previewPath: string | null = null;
  if (arm && cursor) {
    const board = boardRef.current;
    const el = registry.get(`${arm.peerId}:${arm.kind}:${arm.deviceId}`);
    if (board && el) {
      const br = board.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      const a = {
        x: r.left + r.width / 2 - br.left,
        y: r.top + r.height / 2 - br.top,
      };
      previewPath = curve(a, cursor, board.clientWidth / 2);
    }
  }

  let dragOrigin: Pt | null = null;
  if (drag?.active && drag.from) {
    const board = boardRef.current;
    const fromPortId = `${drag.from.peerId}:${drag.from.kind}:${drag.from.deviceId}`;
    const el = registry.get(fromPortId);
    if (board && el) {
      const br = board.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      dragOrigin = {
        x: r.left + r.width / 2 - br.left,
        y: r.top + r.height / 2 - br.top,
      };
    }
  }
  const dragCenterX = boardRef.current ? boardRef.current.clientWidth / 2 : 0;

  const fallbackX = useMotionValue(0);
  const fallbackY = useMotionValue(0);
  const liveX = drag?.x ?? fallbackX;
  const liveY = drag?.y ?? fallbackY;
  const liveDragPath = useTransform([liveX, liveY], (latest) => {
    const [x, y] = latest as number[];
    return dragOrigin ? curve(dragOrigin, { x, y }, dragCenterX) : "";
  });

  const showLiveDrag = Boolean(drag?.active && drag.from && dragOrigin);
  const hasSelection = selectedId !== null;

  return (
    <svg className="absolute inset-0 w-full h-full pointer-events-none z-[1]">
      {previewPath && (
        <path
          d={previewPath}
          fill="none"
          stroke="var(--color-gold)"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeDasharray="5 6"
          opacity="0.75"
          style={{ pointerEvents: "none" }}
        />
      )}
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
      {wires.map((wire) => {
        const isSelected = selectedId === wire.id;
        const strokeWidth = isSelected ? "4" : "2.8";

        return (
          <g key={wire.id}>
            <path
              d={wire.d}
              fill="none"
              stroke="transparent"
              strokeWidth="16"
              style={{ pointerEvents: "stroke", cursor: "pointer" }}
              onClick={() => onSelect(wire.id)}
            />
            <motion.path
              d={wire.d}
              fill="none"
              strokeWidth={strokeWidth}
              strokeLinecap="round"
              initial={reducedMotion ? false : { pathLength: 0 }}
              animate={{ pathLength: 1 }}
              transition={springs.cable}
              style={{
                stroke: wire.color,
                fill: "none",
                strokeDasharray: wire.muted ? "2 5" : undefined,
                opacity: wire.muted
                  ? 0.18
                  : (0.3 + 0.7 * wire.volume) *
                    (hasSelection && !isSelected ? 0.45 : 1),
              }}
            />
            {isSelected && !wire.muted && (
              <path
                d={wire.d}
                fill="none"
                stroke="#fff"
                strokeWidth="2"
                strokeDasharray="1 10"
                strokeLinecap="round"
                opacity="0.65"
                className="[animation:flow_1s_linear_infinite] [pointer-events:none] motion-reduce:animate-none"
                style={{ fill: "none" }}
              />
            )}
            {pulseIds.includes(wire.id) && (
              <motion.circle
                key={`pulse-${wire.id}`}
                cx={wire.sink.x}
                cy={wire.sink.y}
                r={4}
                fill={wire.color}
                initial={{ scale: 0.4, opacity: 0.9 }}
                animate={{ scale: 2.4, opacity: 0 }}
                transition={{ duration: 0.5, ease: "easeOut" }}
                onAnimationComplete={() =>
                  setPulseIds((prev) => prev.filter((id) => id !== wire.id))
                }
                style={{ pointerEvents: "none", transformOrigin: "center" }}
              />
            )}
          </g>
        );
      })}
    </svg>
  );
}
