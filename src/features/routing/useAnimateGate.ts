import { useEffect } from "react";
import { motionValue, type MotionValue } from "motion/react";

const animateGate = motionValue(1);
let subscriberCount = 0;

function syncGate() {
  const active = !document.hidden && document.hasFocus();
  animateGate.set(active ? 1 : 0);
}

export function useAnimateGate(): MotionValue<number> {
  useEffect(() => {
    if (subscriberCount === 0) {
      syncGate();
      document.addEventListener("visibilitychange", syncGate);
      window.addEventListener("blur", syncGate);
      window.addEventListener("focus", syncGate);
    }
    subscriberCount += 1;

    return () => {
      subscriberCount -= 1;
      if (subscriberCount === 0) {
        document.removeEventListener("visibilitychange", syncGate);
        window.removeEventListener("blur", syncGate);
        window.removeEventListener("focus", syncGate);
      }
    };
  }, []);

  return animateGate;
}
