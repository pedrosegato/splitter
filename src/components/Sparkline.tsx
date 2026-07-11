import { motion, useReducedMotion } from "motion/react";
import { durations } from "@/lib/motion";

type SparklineProps = {
  values: number[];
  width?: number;
  height?: number;
  color?: string;
  max?: number;
};

export function Sparkline({
  values,
  width = 80,
  height = 24,
  color = "currentColor",
  max,
}: SparklineProps) {
  const reducedMotion = useReducedMotion();

  if (values.length === 0) {
    return <svg width={width} height={height} aria-hidden="true" />;
  }

  const effectiveMax = max ?? Math.max(...values, 1);
  const points = values
    .map((v, i) => {
      const x = values.length === 1 ? 0 : (i / (values.length - 1)) * width;
      const y = height - (v / effectiveMax) * height;
      return `${x},${y}`;
    })
    .join(" ");

  return (
    <svg width={width} height={height} aria-hidden="true">
      <motion.polyline
        points={points}
        fill="none"
        stroke={color}
        strokeWidth={1.5}
        strokeLinejoin="round"
        strokeLinecap="round"
        initial={reducedMotion ? false : { pathLength: 0, opacity: 0.4 }}
        animate={{ pathLength: 1, opacity: 1 }}
        transition={{ duration: durations.slow, ease: "easeOut" }}
      />
    </svg>
  );
}
