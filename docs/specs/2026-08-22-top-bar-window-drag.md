# Top bar window dragging

## Decision

The frameless desktop window is draggable from empty top-bar chrome. The
left and right titlebar zones expose their unused space as native drag regions,
and Dockview exposes the empty space after each top-edge tab strip in the same
way.

Controls and tab surfaces remain non-draggable. Sidebar toggles and window
buttons must remain clickable, while Dockview tabs retain their own tab/group
drag behavior. Lower split-pane headers do not become window drag handles.

## Verification

- Titlebar CSS tests assert drag coverage for empty chrome and `no-drag`
  exclusions for controls.
- Workbench tests continue to assert that only top-edge empty headers receive
  the native drag marker.
