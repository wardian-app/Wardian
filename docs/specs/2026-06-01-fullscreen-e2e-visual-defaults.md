# Fullscreen E2E Visual Defaults

- **Status:** Implemented
- **Date:** 2026-06-01

## Context

Wardian's automated screenshot and E2E evidence paths historically used a mix of default browser viewport sizes and hand-picked native window sizes such as 1280x720, 1280x1100, 1440x1100, and 980x980. These sizes are useful for deterministic layout coverage, but screenshots used as PR evidence can look slightly different from what users inspect on a normal desktop display.

## Decision

Default visual E2E evidence should use a 1920x1080 desktop viewport or native window unless the test is explicitly about a different size.

Browser E2E now sets Playwright's default viewport to 1920x1080 and keeps environment overrides through `WARDIAN_E2E_VIEWPORT_WIDTH` and `WARDIAN_E2E_VIEWPORT_HEIGHT`.

The real-provider native rendering lab now uses 1920x1080 for its main visual states: `initial`, `settled`, and `wide`. It keeps smaller configured sizes for `narrow`, `resized`, rapid-resize, geometry sweep, outside-terminal parity, and cropped/component evidence because those paths intentionally test wrapping, constrained layouts, terminal geometry, or focused details.

## Consequences

- PR screenshot evidence is closer to a normal fullscreen desktop inspection.
- Existing resize and cramped-layout coverage is preserved through explicit non-fullscreen probes.
- CI and local runs can still override dimensions when a display, runner, or remote desktop cannot provide 1920x1080 reliably.
- Fullscreen defaults may increase screenshot artifact size slightly.

## Verification

The default sizes are covered by focused tests:

- `src/config/e2eViewportDefaults.test.ts` checks the browser E2E viewport default and override knobs.
- `e2e-native/tests/rendering-audit-options.test.mjs` checks the real-provider rendering lab's main visual default sizes and preserves the deliberate resized dimensions.
