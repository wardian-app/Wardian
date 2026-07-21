# macOS Window Integration

## Goal

Make Wardian's installed macOS application feel native at the window boundary:
use the white application icon canvas, keep native traffic lights in the same
titlebar as Wardian's controls, keep the roster toggle away from the window
edge, and relaunch from the updated application bundle after an in-app update.

## Decisions

- Bundle the existing `src-tauri/icons/white/` icon family only from the
  macOS configuration override. Its opaque white canvas prevents macOS from
  compositing the transparent icon against an unintended gray field, while
  Windows and Linux retain their existing transparent icon family.
- Keep the native macOS traffic lights rather than recreating them in HTML.
  The macOS override enables decorations, retains Tauri's `Overlay` titlebar,
  and sets `trafficLightPosition` to `(14, 11)`, centering the controls in the
  36px Wardian titlebar.
- Reserve an 8px trailing inset for the roster toggle. A collapsed macOS
  titlebar zone is 48px wide so the 28px toggle remains inside that inset.
- After `downloadAndInstall` completes outside the Windows installer handoff,
  invoke the backend `restart_app` command. It calls Tauri's native
  `AppHandle::restart`, which restarts the installed macOS bundle after the
  updater has replaced it. The browser-side process plugin is not part of this
  update path.

## Verification

- Unit tests assert the icon bundle and macOS traffic-light configuration.
- Titlebar tests assert the macOS collapsed roster clearance.
- Update tests assert that both automatic and explicit restarts use the native
  backend command and surface command failures.
- The release artifact must still be smoke-tested on both macOS architectures;
  a browser test cannot validate native traffic-light placement or an actual
  application-bundle restart.
