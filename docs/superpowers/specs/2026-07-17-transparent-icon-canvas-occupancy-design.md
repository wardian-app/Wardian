# Transparent Icon Canvas Occupancy Design

## Goal

Make Wardian's transparent app mark read larger in Windows taskbar slots while
keeping the lantern artwork, centering, and cross-platform identity unchanged.

## Decision

Increase the non-transparent artwork in the `src-tauri/icons/transparent/`
asset family by approximately 7 percent around its visual center. The change
applies to every transparent PNG, ICO, ICNS, Windows tile, Android, and iOS
variant so every platform receives the same mark scale. The opaque default and
white icon families are intentionally out of scope.

The current 32px Windows frame occupies 28px by 28px and leaves a 2px border.
The adjusted frame must remain centered, preserve at least a 1px transparent
edge buffer at every small raster size, and never crop or otherwise redraw the
lantern.

## Implementation

Use the transparent master artwork to derive the adjusted variant family with
high-quality resampling. Rebuild container formats from the same adjusted
raster inputs so all ICO and ICNS frames match their PNG counterparts. Do not
change `src-tauri/tauri.conf.json`; it already points at the transparent bundle
assets.

## Verification

Programmatically inspect alpha bounds for every transparent PNG and every ICO
frame to confirm the enlarged occupied bounds stay within their canvases and
retain a non-zero safety margin. Confirm the Tauri bundle configuration keeps
referencing the transparent assets, then run the applicable build validation.

## Scope

This is an asset-only change. It does not modify the icon artwork, application
layout, runtime behavior, or the opaque and white icon families. Windows may
continue to show a cached taskbar image until Wardian is reinstalled or the
pinned taskbar entry is refreshed.
