# macOS native chrome repair

## Context

The `v0.5.0` release used Tauri's macOS Overlay titlebar with a static traffic-light inset. Real-device evidence showed two release-blocking outcomes:

- Native traffic lights were one Wardian titlebar above the sidebar-toggle centerline.
- The green control entered a separate full-screen Space, removing the native controls from the Wardian window.

The release also bundled a white-circle icon family. macOS placed that circle inside its own icon treatment, creating an unwanted white-circle-on-gray result.

An installed-app update was reported as failing around `latest.json`. The release metadata was valid, but the workflow did not verify the public `releases/latest/download/latest.json` redirect after publishing the release.

## Decision

- Keep `titleBarStyle: Overlay` and native traffic lights; do not add custom window buttons or change terminal rendering.
- Set the logical traffic-light inset to `x: 14, y: 11`. In the pinned Tao
  implementation, `y` increases the native titlebar container height from the
  window's upper edge; changing it from `11` to `47` therefore shifted the
  native row down by Wardian's full 36px topbar instead of aligning it with
  that row.
- Set the main `NSWindow` collection behavior to `FullScreenNone` after Tauri creates it. The green traffic light then performs native zoom rather than creating a separate full-screen Space, so the controls remain in Wardian's top-left chrome.
- Use a dedicated full-canvas dark-green `icon.icns` for the macOS bundle. The existing transparent and white-circle families remain unchanged for other targets.
- After publishing a stable release, retry and validate the public `latest.json` route, including both macOS updater entries, before package-workflow dispatch.

## Verification

Automated checks prove the configuration, native AppKit hook, dedicated icon path, and public updater endpoint gate. A release still requires real-Mac evidence:

1. Compare the traffic-light and sidebar-toggle centerlines in a normal window.
2. Press the green button, then restore; controls must remain visible and functional throughout.
3. Confirm the Dock/Finder icon fills the macOS icon treatment without a white circular background.
4. From `/Applications`, update an installed `v0.5.0` build to `v0.5.1`, verify relaunch and the installed version.

## Non-goals

This change does not modify terminal, PTY, renderer, or terminal-geometry behavior.
