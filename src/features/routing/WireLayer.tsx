import { useLayoutEffect, useState, useRef, useCallback } from "react";
import type { StreamSnapshot } from "@/bindings";
import { usePortRegistry } from "./usePortRegistry";
import { curve, streamColor } from "./useWireGeometry";

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
};

export function WireLayer({ boardRef, streams, selectedId, onSelect }: WireLayerProps) {
  const registry = usePortRegistry();
  const [wires, setWires] = useState<ComputedWire[]>([]);
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
        muted: stream.state === "paused",
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

  const hasSelection = selectedId !== null;

  return (
    <svg className="absolute inset-0 w-full h-full pointer-events-none z-[1]">
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
                  ? 0.35
                  : hasSelection && !isSelected
                    ? 0.35
                    : 1,
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
