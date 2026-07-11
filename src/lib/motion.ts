import type { Transition, Variants } from "motion/react";

export const springs = {
  snappy: { type: "spring", stiffness: 520, damping: 32, mass: 0.7 },
  soft: { type: "spring", stiffness: 260, damping: 26 },
  cable: { type: "spring", stiffness: 380, damping: 30, mass: 0.8 },
  cableSettle: { type: "spring", stiffness: 150, damping: 9, mass: 1.1 },
} satisfies Record<string, Transition>;

export const durations = { fast: 0.12, base: 0.22, slow: 0.4 };

export const variants = {
  fadeInUp: {
    hidden: { opacity: 0, y: 8 },
    show: { opacity: 1, y: 0, transition: springs.soft },
  },
  scaleIn: {
    hidden: { opacity: 0, scale: 0.96 },
    show: { opacity: 1, scale: 1, transition: springs.snappy },
  },
  listStagger: {
    hidden: {},
    show: { transition: { staggerChildren: 0.05, delayChildren: 0.02 } },
  },
  listItem: {
    hidden: { opacity: 0, y: 6 },
    show: { opacity: 1, y: 0, transition: springs.soft },
  },
  slide: (dir: 1 | -1): Variants => ({
    enter: { x: dir * 24, opacity: 0 },
    center: { x: 0, opacity: 1, transition: springs.soft },
    exit: { x: dir * -24, opacity: 0, transition: { duration: durations.fast } },
  }),
} satisfies Record<string, Variants | ((dir: 1 | -1) => Variants)>;
