# Dev Home Isolation

## Context

Wardian-managed agent terminals normally inherit `WARDIAN_HOME` from the running production desktop app. Starting a debug desktop build from that shell with `npm run dev` could therefore point the dev app at the same production home. The dev app would then attempt to bind the same control endpoint, use the same remote gateway port, clean up persisted sessions, and restore agents from the production state files.

## Decision

Debug desktop builds treat the default production home as an inherited unsafe value unless explicitly allowed:

- If `WARDIAN_HOME` is unset, the app uses `target/debug/.wardian`.
- If `WARDIAN_HOME` points at a non-production path, the app honors it. Native E2E and manual CLI/app tests can continue to share an explicit isolated home.
- If `WARDIAN_HOME` points at the default production home, the app uses `target/debug/.wardian` instead.
- If `WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME=1`, the debug app may intentionally use the default production home.

The app rewrites its own process `WARDIAN_HOME` to the resolved home before initializing migrations, the database, control endpoint names, remote services, restored agents, or spawned provider processes.

Startup also claims the Wardian control endpoint synchronously during Tauri setup. If another app instance already owns that endpoint for the same home, setup fails before remote gateway startup, stale process cleanup, restore, or state writes.

## Consequences

`npm run dev` is safe by default from a Wardian-managed terminal. Developers who need the CLI and dev app to share state should set an explicit non-production `WARDIAN_HOME` in both terminals. Release builds and the standalone CLI continue to treat `WARDIAN_HOME` as the highest-priority home override.
