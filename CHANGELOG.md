# Changelog

All notable changes to Wardian will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries from `0.3.0` onward are generated automatically by release-please from Conventional Commits. Entries for `0.1.0` through `0.2.1` were backfilled from git history and are thematic summaries rather than exhaustive commit lists.

## [0.2.1] - 2026-03-22

### Features

- **Skill Library**: filesystem-based skill deployment system with a main library view, quick prompt injection, and skill/prompt assignment modals across agents and classes.
- **Class Management**: unified default and custom class lists sharing a single skill management UI.
- **Command Panel**: refined quick-prompt UX and added run actions for library items.
- **Navigation**: centralized sidebar collapse control in the top bar; refactored header layout and tab sizing; updated sidebar icons.
- **Branding**: simplified logo variants and restored SVG asset pipeline.
- **Auto-patch**: Gemini-CLI skills are auto-patched on deployment for consistent behavior.
- **Workflow sidebar**: redesigned with reordering, governance controls, and unit test coverage.

### Bug Fixes

- Restored session-ID retrieval on agent spawn; fixed Windows headless execution.
- Aligned session and resume IDs; fixed binary resolution in agent spawning.
- Resolved lint warnings and UI state-sync issues across agents.
- Auto-created and injected the private agent folder on spawn, ensuring include paths are correct.
- Titlebar: toggle maximize-button icon based on window state; fixed layout collapse on resize.
- Library: resolved `activeTab` jumping; decoupled prompt and skill state in the library store.

### Documentation

- Comprehensive architecture docs, ADR backfill, and user guides landed.
- Renamed ADRs to Specs; updated index and architectural guidelines.

## [0.2.0] - 2026-03-13

### Features

- **Workflow Builder**: initial mockup and then full implementation of the workflow builder, including Loop nodes with cyclic execution, UI validation, run safety, and auto-save on run.
- **Autonomous Nodes**: shell and script workflow nodes with IO separation and security validation.
- **Grid & Dashboard**: mouse-based drag-and-drop with unified selection across main views; views now filter by the active watchlist with synchronized selection.
- **Theming**: light mode, terminal theming, transparent logo, and standardized semantic theming across the app.
- **Agent Classes**: added the Generalist agent class as the default; removed the Designer class.
- **Agent Menus**: aligned menus, added workspace path, and improved watchlist interactions.

### Bug Fixes

- View switching no longer discards terminal history.
- Script error detection hardened in the workflow engine.
- Canvas centering and reset UX improved.

### Refactoring

- Codebase modularized and layout architecture reorganized.
- Terminology standardized (Warden → Coordinator/Agent).

## [0.1.2] - 2026-03-07

### Features

- Agent-panel refactor, standardized `ListEditor` UI, and path validation for agent configurations.

## [0.1.1] - 2026-03-06

### Features

- **Watchlist**: TradingView-style sidebar with mouse-based drag-and-drop, context menus, multi-list membership, and persistent storage.
- **Agent state**: improved Off-state filtering, color semantics, and explicit status text.
- **UI placeholders**: initial main-stage and sidebar view scaffolding.
- **Cross-platform data**: agent data and configurations migrated to `~/.wardian`.

### Bug Fixes

- Terminal: resolved ConPTY deadlocks, maximize bugs, and focus-filter issues; moved to event-based IO with output batching and smart scroll; gated resize/move behind window events with dimension guards.
- Terminal maximization rewritten as CSS fullscreen to preserve scrollback and the global map.
- Watchlist behavior updated to work with the new paused state (closes #36).
- Grid density reworked for 1080p to eliminate 65-column TUI wrap clipping in Gem CLI; restore vertical scroll; maximize window on launch.

### Refactoring

- Renamed Warden → Coordinator (Agent); Workflows (UI) terminology adopted.

### Documentation

- Added README, MIT license, and updated `.gitignore`.

## [0.1.0] - 2026-02-24

Initial public commit. Wardian's first release: a Tauri-based integrated agent environment with a multi-agent grid, terminal panels, and basic orchestration primitives.
