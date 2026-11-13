import { useEffect, useRef, useState } from "react";
/** ~30 updates a second is smooth for slow drift and keeps React work small. */
const FRAME_MS = 33;

const reducedMotion = () => {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
};

/**
 * Seconds of motion so far. Runs only while `running`; when it stops the clock stays where it
 * was, so the map resumes without a jump. Stays at 0 for reduced motion.
 */
export function useMotionClock(running: boolean): number {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!running || reducedMotion()) return;
    let raf = 0;
    let last = performance.now();
    let pending = 0;
    const tick = (t: number) => {
      pending += t - last;
      last = t;
      if (pending >= FRAME_MS) {
        const step = pending / 1000;
        pending = 0;
        setSeconds((s) => s + step);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [running]);
  return seconds;
}

/** Ease in and out (cubic). */
export const easeInOut = (k: number) => (k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2);

/**
 * `target`, approached smoothly over `ms` whenever it changes (e.g. the ship's orbit index: a
 * fractional index flies it along the spiral between two planets). Jumps for reduced motion.
 */
export function useTween(target: number, ms = 1500): number {
  const [value, setValue] = useState(target);
  const current = useRef(target);
  useEffect(() => {
    const from = current.current;
    if (from === target) return;
    if (reducedMotion()) {
      current.current = target;
      setValue(target);
      return;
    }
    const start = performance.now();
    let raf = 0;
    const tick = (t: number) => {
      const k = Math.min(1, (t - start) / ms);
      const v = from + (target - from) * easeInOut(k);
      current.current = v;
      setValue(v);
      if (k < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [target, ms]);
  return value;
}
