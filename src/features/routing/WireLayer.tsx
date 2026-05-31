import { useLayoutEffect, useState, useRef, useCallback } from "react";
import type { StreamSnapshot } from "@/bindings";
import { usePortRegistry } from "./usePortRegistry";
import { curve, streamColor } from "./useWireGeometry";
import { useUiStore } from "@/stores/ui";
import { useThemeStore } from "@/stores/theme";

type WireLayerProps = {
  boardRef: React.RefObject<HTMLDivElement | null>;
  streams: StreamSnapshot[];
  selectedId: number | null;
  onSelect: (streamId: number | null) => void;
};

type ComputedWire = {
  id: number;
  d: string;
  color: string;
  muted: boolean;
  volume: number;
};

export function WireLayer({ boardRef, streams, selectedId, onSelect }: WireLayerProps) {
  const registry = usePortRegistry();
  const arm = useUiStore((s) => s.arm);
  const theme = useThemeStore((s) => s.theme);
  const [wires, setWires] = useState<ComputedWire[]>([]);
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null);
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

  let previewPath: string | null = null;
  if (arm && cursor) {
    const board = boardRef.current;
    const el = registry.get(`${arm.peerId}:src:${arm.deviceId}`);
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
            <path
              d={wire.d}
              fill="none"
              strokeWidth={strokeWidth}
              strokeLinecap="round"
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
          </g>
        );
      })}
    </svg>
  );
}
