import { useCallback, useEffect, useRef, useState } from "react";
import { useMotionValue, type MotionValue } from "motion/react";
import { usePortRegistry } from "./usePortRegistry";
import type { PortRef } from "./resolveConnection";

export type DragState = {
  active: boolean;
  from: PortRef | null;
  x: MotionValue<number>;
  y: MotionValue<number>;
};

type Params = {
  boardRef: React.RefObject<HTMLDivElement | null>;
  onConnect: (from: PortRef, to: PortRef) => void;
};

export function useDragConnect({ boardRef, onConnect }: Params) {
  const registry = usePortRegistry();
  const x = useMotionValue(0);
  const y = useMotionValue(0);
  const [active, setActive] = useState(false);
  const [from, setFrom] = useState<PortRef | null>(null);
  const [hoverPortId, setHoverPortId] = useState<string | null>(null);
  const fromRef = useRef<PortRef | null>(null);

  const toBoard = useCallback(
    (clientX: number, clientY: number) => {
      const board = boardRef.current;
      if (!board) return { bx: clientX, by: clientY };
      const r = board.getBoundingClientRect();
      return { bx: clientX - r.left, by: clientY - r.top };
    },
    [boardRef],
  );

  const startDrag = useCallback(
    (origin: PortRef, e: React.PointerEvent) => {
      fromRef.current = origin;
      setFrom(origin);
      setActive(true);
      const { bx, by } = toBoard(e.clientX, e.clientY);
      x.set(bx);
      y.set(by);
    },
    [toBoard, x, y],
  );

  useEffect(() => {
    if (!active) return;

    const portAt = (clientX: number, clientY: number): string | null => {
      const el = document.elementFromPoint(clientX, clientY);
      const portEl = el?.closest("[data-port-id]") as HTMLElement | null;
      return portEl?.dataset.portId ?? null;
    };

    const onMove = (e: PointerEvent) => {
      const { bx, by } = toBoard(e.clientX, e.clientY);
      x.set(bx);
      y.set(by);
      setHoverPortId(portAt(e.clientX, e.clientY));
    };

    const finish = (e: PointerEvent) => {
      const targetId = portAt(e.clientX, e.clientY);
      const target = targetId ? registry.getRef(targetId) : null;
      const origin = fromRef.current;
      if (origin && target) onConnect(origin, target);
      setActive(false);
      setFrom(null);
      setHoverPortId(null);
      fromRef.current = null;
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
    };
  }, [active, onConnect, registry, toBoard, x, y]);

  const drag: DragState = { active, from, x, y };
  return { drag, startDrag, hoverPortId };
}
