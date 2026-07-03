# Workflow Run Dialog Scroll Bounds

## Context

The workflow launch dialog renders provider selection, role assignments, manual input parameters, and schedule configuration in one modal. Workflows with many input parameters can make that modal taller than the Workflows view. Before this fix, the dialog had no viewport-bound maximum height and no internal scroll region, so the bottom controls could overflow beyond the window.

## Decision

Wardian keeps the launch dialog within the smaller of the browser viewport and the containing Workflows pane. The modal shell is a vertical flex container with hidden outer overflow. The variable form content lives in an internal scroll region, while the title, launch-mode selector, and footer actions stay outside that scrolling body.

This preserves a stable command surface:

- **Run now** and **Schedule** remain reachable even when the workflow has many parameters.
- **Cancel**, **Run**, and **Save schedule** stay visible at the bottom of the dialog.
- The Workflows overlay clips accidental outer overflow so scrolling happens inside the dialog rather than the page.

## Scope

This is a frontend layout fix only. It does not change workflow schema parsing, schedule persistence, provider selection, role assignment semantics, or backend workflow execution.

## Verification

The regression coverage is in `src/features/workflows/RunLaunchDialog.test.tsx`. It renders a workflow with many manual input parameters, switches the dialog to schedule mode, and asserts that the dialog shell is viewport-bounded, the form body scrolls, and the action footer is not part of the scroll region.
