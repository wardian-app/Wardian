# Garden crisp rendering and familiar controls

- **Date:** 2026-09-09
- **Status:** Implemented; local verification and independent review passed
- **Precedence:** Extends and corrects the material-styling spec from the same date; the continuous camera and retained source anchors remain authoritative.

## Experience correction

The operator must recognize Wardian features and read their evidence while moving smoothly through the hierarchy. User review exposed oversized blurred decorations, an unfamiliar record presentation, invented feature names and unexplained global activity modes. The accepted direction retains the tactile hierarchical composition and supports sparse agents as well as hundreds of memories and dozens of conversations.

## Rendering and content

Project leaf detail surfaces into screen pixels instead of magnifying a fixed-size DOM reader. Maintain their world centre and width; the existing continuous height unfolding remains. Text uses normal Wardian reading sizes and constrained columns. Workspace and automation containers retain fixed internal layouts so child world anchors do not drift during zoom. Parent cells fade as a whole before deep record inspection, including the outer shell, decorative marks and shadows. Retain their mounted geometry for reversible navigation. Memory marks use SVG paths instead of tiny CSS borders and pseudo-element strokes.

Use existing Wardian typography, Markdown presentation and metadata conventions for full detail. User-facing headings name the feature; canonical ownership belongs in implementation documentation. Skills, tools, automations, conversations, Inbox, workspace, teams and agents retain their product names. Visual grouping does not establish a parallel domain vocabulary.

## File activity

Remove the global Now / Recent / Branch pills. Workspace contents own an explicit File activity control with Latest 2 turns, Latest 16 turns and All compared changes. These ranges concern attributed file changes, not global time travel. Display the actual comparison baseline and retain changes with uncertain recency. Full-tree browsing disables the activity range and explains why. The overview retains all compared file activity rather than silently adopting a filter hidden in a workspace.

## Acceptance evidence

Exercise continuous wheel entry and return at dense collection scale, confirming parent decorations disappear and restore, source bounds stay unchanged and record text remains crisp. Check workspace range changes, actual baseline, unknown recency and full-tree behavior. Validate keyboard reading, narrow layout, light/dark and reduced motion. Publish refreshed screenshots and an actual-build zoom video with the issue-linked PR after verification and independent review.

### Local evidence

The corrected frontend verifier passed 3,661 tests with one skip across 275 files, plus typecheck, lint, build, all seven debt-budget groups and the remaining frontend gates. The focused Garden browser suite passed all 14 tests without retries, including keyboard reading and the 300-memory / 60-conversation journey.

Independent source review by Wardian-Reviewer approved the corrected diff with zero blocking or non-blocking findings; the verdict was recovered from its conversation transcript because the structured reply command failed. A separate local reviewer also closed with zero blockers. Review led to fixed container child layouts, exact turn-window boundary tests, and complete parent-shell fade and pointer isolation.

The final full DEV browser rerun passed 194 tests with 19 existing skips, zero failures and zero retries (5.1 minutes). An earlier run recorded a browser `ERR_NO_BUFFER_SPACE` while loading a module during setup for one test; that unchanged test passed in isolation and the complete rerun passed. This is not classified as an established base defect. Production evidence includes the 46-second continuous-zoom video and a passing narrow 640 × 700 dense-memory journey, with no scale transform on the leaf reader and zero parent opacity/pointer interception at depth.

The prior hosted coverage timeout in the dense-memory test reproduced locally. Instrumentation identified repeated global accessible-role scans as the test overhead. Scoping search-control queries and verifying the two result buttons by stable references plus explicit accessible names reduced the focused covered search test to 534 ms, preserving all 300 records, behavior assertions and the original timeout. The final test-only diff also received zero-blocker independent review. The separate earlier Graph coverage failure did not reproduce on current or base; no Graph source or test changes were made.

The complete coverage suite subsequently passed all 3,661 tests with one skip across 275 files, including the previously failing Garden search test and unchanged Graph suite.
