# Dismissible Contextual Onboarding

Issue: #735

## Decision

Wardian will use progressive, task-triggered guidance rather than a mandatory
linear tutorial. Contextual tips are visible by default, but users can dismiss
one tip, disable all contextual tips, reset dismissed tips, or open an optional
guided tour at any time.

The existing `settings/onboarding.json` state is the source of truth. The
default must preserve current behavior for existing homes: contextual tips are
enabled until a user disables them.

## Component Plan

### 1. Guidance Preferences And Persistence

- Extend the persisted onboarding state with `contextual_tips_enabled`.
- Preserve backward compatibility with dismissal-only files by defaulting the
  new field to `true`.
- Add backend commands to set the global preference and reset dismissed hint
  IDs without changing the global preference.
- Keep all state scoped to the active `WARDIAN_HOME`.

### 2. Reusable Contextual Hint Primitive

- Update `OnboardingHint` to honor the global preference as well as its
  individual dismissal ID.
- Retain the current compact, inline presentation and accessible dismiss
  button; do not add blocking coach marks.
- Version hint IDs when their guidance changes materially so a revised task can
  be presented deliberately.

### 3. Initial Surface Coverage

- Keep the existing first-agent hint in the spawn panel.
- Add one concise first-use tip to Graph for topology and relationship actions.
- Add one to Command for selected-agent targeting and broadcast scope.
- Add one to the workflow builder for the author-validate-run path.
- Treat future tips as registry entries with the same persistence contract,
  rather than bespoke per-surface state.

### 4. Optional Guided Tour

- Add an in-app, keyboard-accessible tour panel opened from Settings.
- Keep the tour informational and non-blocking: it explains core Wardian work
  loops and points to contextual surfaces and guides, without moving the user
  or changing their workspace.
- The tour remains available even when contextual tips are disabled.

### 5. Controls, Documentation, And Verification

- Add a General Settings control to show or hide contextual tips, reset
  dismissed tips, and open the guided tour.
- Document the controls in the Settings guide and link to the relevant task
  guides from the tour.
- Test Rust persistence and migration defaults, Zustand/UI behavior, individual
  dismissal, global disabling, reset, and keyboard tour handling.

## Non-Goals

- A forced first-run tour.
- Automatic navigation, clicks, or agent actions from tutorial steps.
- Replacing comprehensive guides with in-app copy.
- Analytics or remote tracking of onboarding progress.

## Success Criteria

1. A user can disable all contextual tips from Settings and no tip renders.
2. A dismissed tip remains hidden across restarts until reset.
3. Resetting restores dismissed tips without overriding the user’s global
   preference.
4. The guided tour is discoverable and accessible without interrupting normal
   work.
5. New tips can be added with a stable ID and no new persistence mechanism.
