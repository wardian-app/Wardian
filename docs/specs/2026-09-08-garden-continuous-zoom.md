# Garden Continuous Zoom Contract

- **Status:** Implemented; frontend and browser verification passed; canonical review pending
- **Date:** 2026-09-08
- **Design intent:** [Garden Semantic Zoom Composition](./2026-08-30-garden-semantic-zoom-composition.md)
- **Historical implementation:** [September 7 contract](./2026-09-07-garden-composition-implementation.md)
- **User documentation:** [Garden](../guide/garden.md)
- **Later refinement:** [Object unfolding and bounded rendering](./2026-09-08-garden-object-unfolding.md)

## Correction and Precedence

The user rejected the abrupt switch from the Garden world to a text-heavy DOM
cutaway. The retrieved original references depicted playful agent towns; the
user's Scale of the Universe / infinite-zoom cell-and-organelle analogy is
fundamental interaction intent. Moving closer must reveal what an inhabitant
contains while remaining in the same place. This is not a cosmetic reskin of
the previous detail surface. This document records that reference context in
words; no original reference images are published here.

This contract supersedes the September 7 choices that made continuous morphing
optional, isolated an open DOM interior from the world camera, and required
explicit return instead of reverse-wheel travel. It governs conflicts with
earlier specs. Existing canonical authority, situated automation, attribution,
recency, stale-evidence, and access rules remain applicable.

## One World, One Camera

- Habitat, workstream, composition, and record are bands of one camera's scale.
  Agent circles and their organelle regions retain fixed world geometry.
  Canonical record, workspace, and automation label source bounds retain their
  centre and width. Their reading-plane shape changes as described below;
  spatial continuity does not mean an immutable footprint for every outline.
- Continuous wheel movement reveals and conceals detail through every band.
  Crossing a threshold must not replace the scene with a viewport-sized detail
  panel, teleport the camera, or turn ordinary wheel input into content scroll.
  Reverse wheel retraces the hierarchy without requiring Escape or breadcrumbs.
- Cell silhouettes, enclosing regions, and landmarks maintain visual continuity.
  Detail gains legibility as it approaches; loading, empty, stale, and failed
  regions retain their place. The playful inhabitant and cellular composition
  must remain recognizable rather than becoming an undifferentiated text page.
- DOM may render accessible text and controls using the same source anchors,
  derived reading-plane geometry, and camera transform. Renderer choice does
  not relax spatial continuity.
- Selection alone does not move the camera. Selected-object wheel travel can
  reveal its descendants; empty-space zoom does not invent a selected target.
  Threshold hysteresis may prevent flicker but must not trap inward or outward
  travel. Thresholds remain implementation tuning within this contract.

### Source Anchors, Reading Planes, and Entry

Canonical record, workspace, and automation labels keep their source centre
and width fixed. As the projected source width moves from **180 to 540 CSS
pixels**, the outline height smoothly interpolates from the source height to
**0.78 times its width**; zooming out reverses that change. This creates a
reading plane around the same anchor. Keeping a thin label's aspect ratio at
all scales left a workspace link in Ports as an unusable roughly 10-pixel-high
strip. The height morph is an explicit correction, not a claim of strictly
immutable world bounds. Agent circles and organelle-region geometry remain fixed.

Every non-district **Enter** targets a minimum projected width of **720 CSS
pixels**, including agents, workspaces, automations, and records. District entry
continues to fit its district. On a short or narrow viewport, the outer plane
or agent membrane may extend beyond the viewport. The reading column is
constrained to the available viewport width and height, with text flowing
inside it without moving the source anchor. Pan and zoom reveal the surrounding
plane while preserving geography; this remains part of the same camera scene.

Keyboard focus remains available on the coarse cell before its inner reading
controls become legible. Enter uses the same camera to approach that cell;
inner controls become available at readable scale. A peer link in **Ports**
travels to the peer's canonical cell position, rather than nesting a duplicate
peer cell inside the originating agent. The return path retains the origin.

## Input and Reading

| Input | Required behavior |
| --- | --- |
| Click / Space | Select and explain without moving the camera |
| Wheel / trackpad wheel | Zoom the same camera in or out across all bands, including over record content |
| Double-click / Enter / touch Enter action | Animate that camera toward the selected object's next meaningful bounds |
| Escape / ancestor breadcrumb | Animate that camera toward the enclosing or selected ancestor context; preserve a reversible path |
| Alt + wheel over overflowing content | Explicitly scroll that content without zooming the camera |
| Tab to a named reading region, then keyboard scrolling | Scroll with arrows, Page Up/Page Down, and Home/End; do not also pan the world |

Pointer zoom remains anchored at the pointer. Keyboard object traversal and
canvas-focused panning remain distinguishable from reading-area scrolling.
Named reading regions participate in Tab focus and provide arrow and
Page Up/Page Down scrolling without panning the camera.
Visible hints must explain ordinary wheel zoom, Alt + wheel reading, and
keyboard access. Reduced motion suppresses travel animation while preserving
camera destinations, object anchors, focus, and reversible navigation.

Canonical actions such as **Open agent session**, **Open in Library**, and
**Open file** remain explicit exits to their owning surfaces. Garden is a lens
over canonical records, not a new editor or source of truth. Returning from a
canonical destination preserves Garden's camera, selection, and hierarchy.

## Legacy Trail Recovery

Older saved trails may lack source bounds. When a record cannot be resolved
back to a world anchor, recover to the nearest resolvable agent or district
ancestor and show a notice explaining the recovery. Do not invent a record
position or leave the user in an inaccessible reading plane. Resolving legacy
state must preserve a usable camera and navigation path.

## Validation Evidence and Remaining Gates

The integration owner reports the following evidence for the latest frozen
source, including the reading-plane, legacy-recovery, and drag changes:

- **Frontend verifier:** exit **0**; **3,593 tests passed, one skip, 270 files**.
  TypeScript, lint, production build, and all repository frontend checks passed.
- **Focused browser suite:** **15 passed**.
- **Full browser suite:** **passed**. The directly verified `.last-run.json`
  records `status: "passed"` and `failedTests: []`; the final summary reports
  **191 passed, 18 existing skips** in **6.4 minutes**, with zero failures and
  no retries.
- **Independent review:** Harvey reports **zero blocking findings**, including
  reading, narrow layouts, legacy recovery, and drag. The canonical
  Wardian-Reviewer request remains pending because that reviewer was unavailable;
  the independent verdict does not claim completion of the canonical review.
  Non-blocking follow-ups are tracked in
  [#1224](https://github.com/wardian-app/Wardian/issues/1224) and
  [#1225](https://github.com/wardian-app/Wardian/issues/1225).
- **Visual evidence:** a [99-second continuous-zoom video](https://github.com/wardian-app/Wardian/releases/download/_gh-attach-assets/garden-continuous-zoom.mp4)
  is uploaded. Both existing reader images were refreshed from the latest
  reviewed screenshots: `docs/assets/screenshots/garden/agent-cutaway.png` and
  `docs/assets/screenshots/garden/workstream-inhabitants.png`.

An earlier RemoteMobileApp timeout did not reproduce in isolated current,
isolated base, or full-base checks. The final full-current run passed without
changes addressing it. This is a non-reproducing timeout, not an established
pre-existing failure.

These results do not establish native/provider behavior or a complete
accessibility audit. September 7 results remain historical evidence for the
superseded implementation. Canonical review remains pending; no merge-readiness
or overall approval is claimed here.

The acceptance criteria for this revision remain:

1. Wheel-only travel reaches a readable nested record from Habitat and reverses
   back through every band without scene replacement or an explicit-return trap.
2. Enter, Escape, and breadcrumbs use the same camera and world anchors;
   repeated threshold crossings, interrupted travel, and return preserve context.
3. Ordinary wheel over a record zooms; Alt + wheel scrolls content; focused
   reading areas support keyboard scrolling without moving the world.
4. Agents, workspace contents, and situated automation records remain legible
   and spatially connected at representative desktop and narrow sizes, with
   long content, missing evidence, keyboard focus, and reduced motion. Explicit
   non-district entry preserves the 720-pixel minimum projected width on short
   screens while reading columns fit available width and height. Outline height
   morphs smoothly across 180–540 pixels without moving source centres or widths;
   agent and organelle geometry stays fixed. Cropped outer planes remain
   reachable by pan/zoom. Coarse cells support keyboard entry, and peer Ports
   crosslinks retain canonical cell positions.
5. Fresh screenshots and interaction evidence establish the playful world and
   cell interior continuity. Still images alone do not prove reversible travel.
   Canonical action return paths also need regression evidence.
6. Legacy trails without bounds recover unresolved records to the nearest
   resolvable agent or district and explain the recovery in a visible notice.

Use unit checks for camera math and input routing, browser checks for rendered
travel and reading behavior, and native/provider checks only for claims that
require those layers. Record actual outcomes and remaining limits separately.
The refreshed reader images document the implemented scene; original inspiration
images are not product evidence. Public guide links remain in the guide tree,
and release notes follow the owning feature's delivery process.
