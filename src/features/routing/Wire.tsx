import { memo, useEffect } from "react";
import {
  motion,
  animate,
  useMotionValue,
  useSpring,
  useTransform,
  useTime,
  usePresence,
} from "motion/react";
import { springs } from "@/lib/motion";
import { cable, sagFor, type Pt } from "./useWireGeometry";
import { useAnimateGate } from "./useAnimateGate";

type WireProps = {
  id: number;
  a: Pt;
  b: Pt;
  color: string;
  muted: boolean;
  volume: number;
  selected: boolean;
  hasSelection: boolean;
  reducedMotion: boolean;
  onSelect: (id: number) => void;
};

function WireImpl({
  id,
  a,
  b,
  color,
  muted,
  volume,
  selected,
  hasSelection,
  reducedMotion,
  onSelect,
}: WireProps) {
  const [isPresent, safeToRemove] = usePresence();

  const ax = useMotionValue(a.x);
  const ay = useMotionValue(a.y);
  const bx = useMotionValue(b.x);
  const by = useMotionValue(b.y);
  useEffect(() => {
    ax.set(a.x);
    ay.set(a.y);
    bx.set(b.x);
    by.set(b.y);
  }, [a.x, a.y, b.x, b.y, ax, ay, bx, by]);

  const restSag = sagFor(a, b);
  const sag = useSpring(reducedMotion ? restSag : 0, springs.cableSettle);
  useEffect(() => {
    if (isPresent) sag.set(restSag);
  }, [isPresent, restSag, sag]);

  const time = useTime();
  const gate = useAnimateGate();
  const swayAmp = 2.5;
  const phase = id * 1.3;

  const swayInputs = reducedMotion
    ? [ax, ay, bx, by, sag]
    : [ax, ay, bx, by, sag, time, gate];

  const d = useTransform(swayInputs, (v) => {
    const [axv, ayv, bxv, byv, s] = v as number[];
    const eff = reducedMotion
      ? s
      : s + swayAmp * (v[6] as number) * Math.sin((v[5] as number) / 3000 + phase);
    return cable({ x: axv, y: ayv }, { x: bxv, y: byv }, eff);
  });

  const baseOpacity = muted
    ? 0.18
    : (0.3 + 0.7 * volume) * (hasSelection && !selected ? 0.45 : 1);

  const opacity = useMotionValue(reducedMotion ? baseOpacity : 0);
  useEffect(() => {
    if (!isPresent) return;
    const controls = animate(opacity, baseOpacity, springs.soft);
    return () => controls.stop();
  }, [isPresent, baseOpacity, opacity]);

  useEffect(() => {
    if (isPresent) return;
    sag.set(restSag * 1.7);
    const controls = animate(opacity, 0, { duration: 0.32, ease: "easeIn" });
    controls.then(() => safeToRemove?.());
    return () => controls.stop();
  }, [isPresent, opacity, sag, restSag, safeToRemove]);

  const shadowOpacity = useTransform(opacity, (o) => o * 0.5);
  const sheenOpacity = useTransform(opacity, (o) => o * 0.22);
  const strokeWidth = selected ? 4 : 2.8;

  return (
    <g>
      <motion.path
        d={d}
        fill="none"
        stroke="#000"
        strokeWidth={strokeWidth + 3}
        strokeLinecap="round"
        style={{ opacity: shadowOpacity, translateY: 3, filter: "blur(2.5px)" }}
      />
      <motion.path
        d={d}
        fill="none"
        stroke={color}
        strokeWidth={strokeWidth}
        strokeLinecap="round"
        strokeDasharray={muted ? "2 5" : undefined}
        initial={reducedMotion ? false : { pathLength: 0 }}
        animate={{ pathLength: 1 }}
        transition={springs.cable}
        style={{ opacity }}
      />
      <motion.path
        d={d}
        fill="none"
        stroke="#fff"
        strokeWidth={1}
        strokeLinecap="round"
        style={{ opacity: sheenOpacity, translateY: -1 }}
      />
      {!reducedMotion && (
        <motion.path
          d={d}
          fill="none"
          stroke={color}
          strokeWidth={10}
          strokeLinecap="round"
          initial={{ opacity: 0.5 }}
          animate={{ opacity: 0 }}
          transition={{ duration: 0.55, ease: "easeOut" }}
          style={{ filter: "blur(3px)" }}
        />
      )}
      {selected && !muted && (
        <motion.path
          d={d}
          fill="none"
          stroke="#fff"
          strokeWidth={2}
          strokeDasharray="1 10"
          strokeLinecap="round"
          opacity={0.65}
          className="[animation:flow_1s_linear_infinite] [pointer-events:none] motion-reduce:animate-none"
          style={{ fill: "none" }}
        />
      )}
      <motion.path
        d={d}
        fill="none"
        stroke="transparent"
        strokeWidth={16}
        style={{ pointerEvents: "stroke", cursor: "pointer" }}
        onClick={() => onSelect(id)}
      />
      {!reducedMotion && isPresent && (
        <motion.circle
          cx={b.x}
          cy={b.y}
          r={4}
          fill={color}
          initial={{ scale: 0.4, opacity: 0.9 }}
          animate={{ scale: 2.4, opacity: 0 }}
          transition={{ duration: 0.5, ease: "easeOut" }}
          style={{ pointerEvents: "none", transformOrigin: "center" }}
        />
      )}
    </g>
  );
}

export const Wire = memo(WireImpl);
