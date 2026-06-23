import { describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useGardenPulse } from "./useGardenPulse";

describe("useGardenPulse", () => {
  it("returns a steady 1 and schedules no animation when inactive", () => {
    const raf = vi.spyOn(globalThis, "requestAnimationFrame");
    const { result } = renderHook(() => useGardenPulse(false));
    expect(result.current).toBe(1);
    expect(raf).not.toHaveBeenCalled();
    raf.mockRestore();
  });

  it("schedules animation frames when active", () => {
    const raf = vi.spyOn(globalThis, "requestAnimationFrame").mockReturnValue(1 as unknown as number);
    renderHook(() => useGardenPulse(true));
    expect(raf).toHaveBeenCalled();
    raf.mockRestore();
  });
});
