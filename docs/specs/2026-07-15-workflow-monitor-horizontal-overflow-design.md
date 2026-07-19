# Workflow Monitor Horizontal Overflow Design

## Problem

The workflow monitor renders activity and history as a six-column operational table. Its current responsive switch uses the browser viewport through Tailwind's `md:` breakpoint. A Workflows surface can occupy a narrow workbench pane while the desktop viewport remains wide, so the table keeps its wide layout and is clipped by overflow-hidden ancestors.

## Interaction Design

The monitor statistics, error state, Activity header, and filter controls remain fixed to the pane width. The activity and history content below the header becomes one shared two-axis scroll region. When the table fits, no horizontal scrollbar appears. When it does not fit, the user can scroll horizontally without widening the workbench pane or moving the monitor chrome.

All activity sections share the same horizontal scroll position. Rows and their action column move together, preserving column alignment and avoiding nested scrollbars on individual sections or rows. Existing vertical history scrolling and virtualization remain owned by the same scroll region.

## Layout

The activity table receives a 960-pixel intrinsic minimum width derived from its six existing columns, gaps, padding, and action controls. The grid no longer depends on a viewport breakpoint to decide whether those columns exist. Wide panes continue to distribute available space through the flexible workflow-name column; narrow panes expose the table's overflow through the shared scroll region.

The empty activity state remains pane-width content because it does not need the table's intrinsic width.

## Accessibility

The scroll region retains its existing test identity and gains an accessible label describing it as workflow activity. Keyboard and touchpad scrolling use native browser overflow behavior. Buttons, labels, and focus order remain unchanged.

## Verification

- Unit coverage verifies that the history region supports horizontal and vertical overflow and that table content owns a stable minimum width.
- Browser coverage opens Workflows Monitor in a narrow workbench pane and verifies that the activity table overflows inside its own scroller without increasing the pane or application width.
- Existing workflow monitor tests continue to cover filtering, virtualized history, and row actions.

## Scope

This change does not redesign monitor columns, add custom scrollbar controls, alter workflow data, or change the Workflows toolbar, run drawer, builder, or observe modes.
