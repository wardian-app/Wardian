import { useEffect, useRef, useState } from "react";

/**
 * Returns a gentle 1.0 +/- breathing scale while `active`, and a flat 1 otherwise.
 * No requestAnimationFrame loop runs while inactive, protecting idle CPU.
 */
export function useGardenPulse(active: boolean): number {
  const [scale, setScale] = useState(1);
  const frame = useRef<number | null>(null);
  const startRef = useRef<number>(0);

  useEffect(() => {
    if (!active) {
      setScale(1);
      return;
    }
    let mounted = true;
    const tick = (now: number) => {
      if (!mounted) return;
      if (!startRef.current) startRef.current = now;
      const elapsed = (now - startRef.current) / 1000;
      setScale(1 + 0.08 * Math.sin(elapsed * Math.PI));
      frame.current = requestAnimationFrame(tick);
    };
    frame.current = requestAnimationFrame(tick);
    return () => {
      mounted = false;
      startRef.current = 0;
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    };
  }, [active]);

  return scale;
}
