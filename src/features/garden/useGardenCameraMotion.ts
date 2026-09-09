import { useCallback, useEffect, useRef } from "react";
import type { GardenCamera } from "./garden.types";
import { interpolateCamera } from "./gardenSpatialZoom";

/** Explicit navigation animates the same camera used by wheel and pan gestures. */
export function useGardenCameraMotion(camera: GardenCamera | undefined, publish: (camera: GardenCamera) => void) {
  const live = useRef(camera);
  live.current = camera;
  const frame = useRef<number | null>(null);
  const cancel = useCallback(() => {
    if (frame.current !== null) cancelAnimationFrame(frame.current);
    frame.current = null;
  }, []);
  useEffect(() => cancel, [cancel]);
  const move = useCallback((target: GardenCamera, size: { width: number; height: number }, onComplete?: () => void) => {
    cancel();
    const from = live.current;
    if (!from || window.matchMedia("(prefers-reduced-motion: reduce)").matches) { publish(target); onComplete?.(); return; }
    const started = performance.now();
    const tick = (now: number) => {
      const t = Math.min(1, (now - started) / 650);
      const eased = t * t * (3 - 2 * t);
      publish(interpolateCamera(from, target, eased, { x: size.width / 2, y: size.height / 2 }));
      frame.current = t < 1 ? requestAnimationFrame(tick) : null;
      if (t === 1) onComplete?.();
    };
    frame.current = requestAnimationFrame(tick);
  }, [cancel, publish]);
  return { move, cancel };
}
