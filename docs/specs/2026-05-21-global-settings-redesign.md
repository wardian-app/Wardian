# Global Settings Redesign

- **Status:** Implemented
- **Date:** 2026-05-21
- **Issue:** [#184](https://github.com/wardian-app/Wardian/issues/184)
- **Related:** [#336](https://github.com/wardian-app/Wardian/issues/336)

## Context and Problem Statement

Wardian's current Settings surface has outgrown the left sidebar panel. It now
mixes update controls, theme selection, terminal appearance, shell selection,
agent runtime defaults, Codex policy, and Gemini patch utilities in a narrow
column. Some preferences persist through browser localStorage, while shell and
runtime defaults persist through Rust-backed JSON files. This split makes
settings harder to inspect, migrate, test, and extend.

Settings should feel like mature desktop application infrastructure: searchable,
grouped, resettable, and backed by visible files under Wardian home. The first
redesign should handle global app settings only. Project, workspace, or
agent-scoped settings are explicitly deferred to #336.

## Decision

Wardian will replace the sidebar Settings panel with a rail-launched,
near-full-screen Settings modal. The gear icon remains on the primary icon rail,
but opening Settings does not change the selected sidebar tab, collapse/expand
the sidebar, select a different main view, or clear agent selection. Closing
Settings returns the user to the exact app context underneath.

The Settings modal is an app-level preferences surface, not a small dialog. It
uses a dense IDE-style layout:

- fixed category navigation inside the modal
- search input at the top of the content area
- grouped rows for related settings
- clear setting labels, descriptions, controls, and apply-timing notes
- modified/default indicators
- reset-to-default actions
- validation messages near the affected setting
- close button and keyboard close behavior

The modal should occupy most of the app window on desktop and become full-screen
on small windows. It must have one predictable internal scroll area for settings
content. It should not close from an outside click when there are unsaved,
invalid, or validating settings.

## Architecture

Settings will be registry-driven. A settings registry defines the renderable and
behavioral metadata for each setting:

- stable key
- label
- description
- category and group
- search keywords
- control type
- default value
- persistence domain
- validation behavior
- reset behavior
- apply timing, such as immediate, future launches, restart, or manual action
- optional documentation link

The UI renders settings from this registry instead of growing one large
hand-authored form. Feature-specific settings can register new sections without
editing a monolithic settings component.

The frontend may keep a cached settings store for responsive rendering and
optimistic UI state, but durable settings should be loaded from and saved through
backend commands. Rust remains the source of truth for file paths, defaults,
normalization, validation, and migration.

## Persistence Model

All settings in scope for #184 are global. They persist under
`<WARDIAN_HOME>/settings/` in domain files rather than one large file.

Initial domains:

- `settings/app.json`: app preferences such as theme, terminal font size,
  terminal font family, Gemini auto-patch preference, and similar UI-level
  preferences currently stored in browser localStorage.
- `settings/shell.json`: existing shell, provider default, session persistence,
  and Codex runtime policy settings. This file remains the runtime domain for
  compatibility with existing backend code and user data.

Future domains can be added as settings grow, for example provider-specific or
notification settings. Domain files should use explicit schema versions when
needed and should be safe to inspect on disk.

The migration must preserve existing user values from both localStorage and
`settings/shell.json`. Existing shell settings must not be renamed or moved in a
way that breaks current runtime code. If migration writes a new file, it should
normalize values, keep defaults explicit where useful, and avoid deleting old
data until the new settings have been saved successfully.

## Save and Apply Semantics

Settings use mixed save semantics based on risk:

- Low-risk display preferences, such as theme or terminal font size, save and
  apply immediately.
- Runtime settings that affect future provider launches, such as shell,
  provider default, session persistence, or Codex policy, save through backend
  validation and clearly state that they affect future launches or resumes.
- Settings that require restart or manual action must display that apply timing
  in the row.

Every setting row should know whether it is at its default value. Reset actions
should save through the same validation path as ordinary edits.

## UI Categories

The first implementation should organize current settings into categories rather
than preserve the current vertical order:

- **General**: app version, updates, and broad app preferences.
- **Appearance**: app theme and display preferences.
- **Terminal**: terminal font size, terminal font family, and default shell.
- **Agent Runtime**: default provider, regular agent session persistence, and
  Codex runtime defaults.
- **Provider Utilities**: Gemini patch controls and other provider-specific
  maintenance actions.
- **Advanced**: diagnostics, settings file access, and future low-level controls.

Category names can be adjusted during implementation if the resulting UI reads
more clearly, but the design should avoid dumping unrelated settings into one
advanced bucket.

## Accessibility and Interaction Requirements

The modal must behave like a proper application dialog:

- initial focus lands on the search input or modal heading
- `Esc` closes the modal when there are no blocking dirty or invalid changes
- focus returns to the rail gear after close
- focus is trapped while the modal is open
- background app content is not interactive while the modal is open
- controls are keyboard reachable in category-nav and content order
- validation errors are associated with the relevant controls
- category navigation and search do not cause layout jumps

The settings gear should show an active state while the modal is open, but this
active state is independent from the selected sidebar tab.

## Scope

In scope:

- remove Settings from the left sidebar content pane
- add rail-launched Settings modal state
- implement registry-driven global settings UI
- migrate localStorage-backed settings into file-backed app settings
- preserve and reuse existing `settings/shell.json` runtime settings
- add search, category navigation, reset/default state, validation feedback, and
  apply-timing labels
- update tests and user documentation

Out of scope:

- project, workspace, agent-class, or per-agent settings scopes
- scoped precedence rules
- trust-gated project settings
- secrets storage or credential management
- full import/export of settings bundles

Scoped settings rules are tracked separately in #336.

## Consequences

- **Positive:** Settings no longer competes for space in the sidebar and can
  scale as Wardian adds more runtime, provider, and app preferences.
- **Positive:** File-backed global settings align with Wardian's
  Markdown-as-Truth and inspectable-on-disk principles.
- **Positive:** A registry-driven UI reduces future settings drift and makes
  search, reset, defaults, and docs links consistent.
- **Positive:** Keeping `settings/shell.json` avoids unnecessary churn in
  existing runtime code and user data.
- **Negative:** A near-full-screen modal requires careful focus trapping,
  background inertness, scroll containment, and responsive testing.
- **Negative:** Mixed save semantics are more complex than a single Save button,
  but they better match the risk profile of different settings.
- **Negative:** Moving localStorage preferences into files requires a one-time
  migration path and compatibility tests.

## Verification

Frontend verification should cover:

- the rail gear opens Settings without changing the selected sidebar tab
- closing Settings restores the prior main app context
- keyboard close and focus return behavior
- focus trap and background inertness
- category navigation and search filtering
- reset-to-default behavior
- validation and error states for runtime settings
- localStorage migration for app preferences

Backend verification should cover:

- app settings load/save/default behavior
- shell settings compatibility with existing `settings/shell.json`
- migration from old localStorage-facing defaults into file-backed app settings
- validation failures do not corrupt existing settings files

Documentation should update the Settings guide and note that #184 implements
global settings only. Any future scoped settings behavior should reference #336.
