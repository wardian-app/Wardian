import { describe, expect, it } from "vitest";

import {
  measureHint,
  measureLabel,
  measureShortLabel,
  MEASURE_GROUPS,
  formatCount,
  formatDuration,
  formatPercent,
  formatBucketLabel,
  formatMeasureValue,
  cellIntensity,
  UNREPORTED,
} from "./telemetryFormat";

describe("formatDuration", () => {
  it("keeps a short real span distinguishable from no activity", () => {
    // Rounding forty seconds to "0m" would make a working agent look idle.
    expect(formatDuration(40_000)).toBe("<1m");
    expect(formatDuration(0)).toBe("0m");
  });

  it("climbs to the coarsest unit that still says something", () => {
    expect(formatDuration(9 * 60_000)).toBe("9m");
    expect(formatDuration(90 * 60_000)).toBe("1h 30m");
    expect(formatDuration(2 * 60 * 60_000)).toBe("2h");
    expect(formatDuration(26 * 60 * 60_000)).toBe("1d 2h");
    expect(formatDuration(48 * 60 * 60_000)).toBe("2d");
  });

  it("treats nonsense as no time rather than throwing", () => {
    expect(formatDuration(Number.NaN)).toBe("0m");
    expect(formatDuration(-5)).toBe("0m");
  });
});

describe("formatCount", () => {
  it("distinguishes an unreported measure from a reported zero", () => {
    // The whole reason token fields are nullable: antigravity reports nothing,
    // and showing that as 0 ranks it the cheapest provider.
    expect(formatCount(null)).toBe(UNREPORTED);
    expect(formatCount(undefined)).toBe(UNREPORTED);
    expect(formatCount(0)).toBe("0");
  });

  it("abbreviates only once the full number stops being readable", () => {
    expect(formatCount(999)).toBe("999");
    expect(formatCount(1_000)).toBe("1.0k");
    expect(formatCount(831_424)).toBe("831.4k");
    expect(formatCount(2_500_000)).toBe("2.5M");
    expect(formatCount(3_100_000_000)).toBe("3.1B");
  });

});

describe("formatPercent", () => {
  it("keeps an unreported limit blank rather than showing 0%", () => {
    // A provider that published no usage figure has not published 0% usage.
    expect(formatPercent(null)).toBe(UNREPORTED);
    expect(formatPercent(31.75)).toBe("32%");
    expect(formatPercent(0)).toBe("0%");
  });
});

describe("formatMeasureValue", () => {
  it("renders a duration as time and everything else as a count", () => {
    // 2,400,000 active milliseconds formatted as "2.4M" would be true and
    // useless. The unit belongs to the measure, not to the number.
    expect(formatMeasureValue("active_ms", 2_400_000)).toBe("40m");
    expect(formatMeasureValue("total_tokens", 2_400_000)).toBe("2.4M");
    expect(formatMeasureValue("files", 12)).toBe("12");
    // A ratio without its sign reads as a count of 88.
    expect(formatMeasureValue("cache_hit_rate", 88)).toBe("88%");
  });
});

describe("cellIntensity", () => {
  it("keeps an empty cell exactly empty", () => {
    // The one distinction a heatmap must never blur: nothing happened has to
    // look different from a little happened.
    expect(cellIntensity(0, 100)).toBe(0);
    expect(cellIntensity(5, 0)).toBe(0);
    expect(cellIntensity(-1, 100)).toBe(0);
  });

  it("keeps quiet cells visible against a dominant one", () => {
    // A linear ramp would render 1% of the busiest hour as invisible, and token
    // counts across a habitat span orders of magnitude. The curve lifts the
    // quiet end without reordering anything.
    const linear = 1 / 100;
    const curved = cellIntensity(1, 100);
    expect(curved).toBeGreaterThan(linear);
    expect(curved).toBeLessThan(1);
    expect(cellIntensity(100, 100)).toBe(1);
  });

  it("preserves ordering", () => {
    expect(cellIntensity(10, 100)).toBeLessThan(cellIntensity(50, 100));
  });

  it("clamps a value above the maximum rather than overshooting", () => {
    expect(cellIntensity(500, 100)).toBe(1);
  });
});

describe("formatBucketLabel", () => {
  it("labels hour and day columns differently", () => {
    const hour = formatBucketLabel("2026-08-13T18:00:00.000Z", "hour");
    const day = formatBucketLabel("2026-08-13T18:00:00.000Z", "day");
    expect(hour).toMatch(/\d{2}/);
    expect(day).not.toBe(hour);
  });

  it("is blank rather than 'Invalid Date' for an unparseable bucket", () => {
    expect(formatBucketLabel("nonsense", "day")).toBe("");
  });
});

describe("measure labels", () => {
  /** Every measure `Measure::parse` accepts, from `crates/wardian-core/src/telemetry/matrix.rs`. */
  const BACKEND_MEASURES = [
    "active_ms",
    "turns",
    "fresh_tokens",
    "cached_tokens",
    "cache_write_tokens",
    "output_tokens",
    "reasoning_tokens",
    "total_tokens",
    "cache_hit_rate",
    "files",
    "lines_added",
    "lines_removed",
    "lines_changed",
  ];

  it("offers every measure the backend can plot", () => {
    const offered = MEASURE_GROUPS.flatMap((group) => group.measures.map((option) => option.id));
    expect(offered.slice().sort()).toEqual(BACKEND_MEASURES.slice().sort());
  });

  it("names the quantity without inventing vocabulary", () => {
    // "Fresh input" was the original label, and nothing on the surface said what
    // made input fresh — that it was not served from the provider's cache, which
    // is the entire distinction between this measure and the one beside it.
    expect(measureLabel("fresh_tokens")).toBe("New input");
    expect(measureLabel("cached_tokens")).toBe("Cached input");
    expect(measureHint("fresh_tokens")).toContain("cache");
  });

  it("gives every measure a definition and a compact form", () => {
    for (const measure of BACKEND_MEASURES) {
      expect(measureHint(measure), measure).not.toBe("");
      // Eighty pixels of header, at 10px type.
      expect(measureShortLabel(measure).length, measure).toBeLessThanOrEqual(9);
    }
  });

  it("falls back to the raw id rather than rendering undefined", () => {
    expect(measureLabel("not_a_measure")).toBe("not_a_measure");
    expect(measureHint("not_a_measure")).toBe("");
  });
});
