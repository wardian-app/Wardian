# Wardian CLI and `wardian agent` Command

- **Status:** Implemented
- **Date:** 2026-04-21

## Context and Problem Statement

Wardian has no command-line interface. Every interaction — spawning agents, inspecting state, running workflows — goes through the Tauri GUI. This forces a specific problem: the primary users of Wardian are *agents*, not humans, and agents cannot usefully drive a graphical app. They need a textual surface to introspect themselves, discover peers, and (eventually) act on the system they live inside.

[Agent Identity and Status Tracking](./2026-04-20-agent-identity-and-status-tracking.md) already lays the groundwork for this by making the Rust backend the source of truth for agent identity and status via `~/.wardian/state.db`, and by injecting `WARDIAN_SESSION_ID` into every spawned agent's environment. [Release System](./2026-04-19-release-system.md) reserves a slot for a `wardian-cli` binary in the release pipeline and documents its intended bundle location. What is missing is the CLI itself.

This spec defines the first slice: a `wardian` command distributed alongside the GUI, sharing Rust code with the Tauri app through a workspace refactor, always on the user's `PATH`, and exposing one noun — `wardian agent` — for self- and peer-introspection. Mutating commands (`spawn`, `send`, `kill`), auto-update, and distribution wrappers (npm/pip/cargo) are explicitly deferred to follow-up specs. The command surface is designed so those follow-ups slot in without restructuring.

## Proposed Decision

### Scope

**In scope:**

- Cargo workspace refactor extracting `wardian-core` (shared models, DB access, identity resolution) from the existing Tauri crate while keeping the Tauri app in `src-tauri/`.
- New `wardian-cli` binary crate producing the CLI implementation binary.
- `wardian agent` command surface: self-lookup, peer lookup, list.
- JSON-first output with schema versioning; human-readable `--pretty` variant.
- Structured error envelope with stable `code`, human `message`, actionable `hint`, optional `details`, and a defined exit-code table.
- Per-user install to `~/.wardian/bin/wardian` on macOS/Linux and `%USERPROFILE%\.wardian\bin\wardian.cmd` on Windows, with the GUI adding that directory to user `PATH` on first launch.
- Forward-compatible `--scope` flag as a placeholder for future team / workspace / comm-graph scoping.

**Out of scope (deferred):**

- Shared auto-updater (`wardian update`) — separate spec, immediate follow-up.
- Mutating agent commands (`wardian agent spawn`, `send`, `kill`, `logs`).
- Mutating IPC between CLI and running GUI.
- Other top-level namespaces (`wardian workflow`, `wardian library`, `wardian doctor`).
- Distribution wrappers: npm, pip, cargo packages that download the binary from GitHub Releases.

### Component 1 — Workspace Refactor

The repository moves from a single `src-tauri/` crate to a Cargo workspace at repo root. Proposed layout:

```
Cargo.toml                 # workspace manifest
src-tauri/                 # existing Tauri app crate
crates/
  wardian-core/            # shared library
  wardian-cli/             # new binary crate
```

**`wardian-core`** owns the code that must not drift between the GUI and CLI:

- `models/` — the existing DTOs (agent rows, status enums, events). Moved verbatim.
- `db/` — `rusqlite` wrappers, migrations, connection helpers, WAL-mode setup.
- `identity/` — resolution logic (`resolve_self_from_env`, `resolve_by_name_or_uuid`, `list_scoped`).
- `paths/` — the `~/.wardian/` directory layout constants (state DB path, agents dir, bin dir).

**`src-tauri`** keeps everything Tauri-specific: `commands/`, `providers/`, `workflow_engine/`, `manager.rs`, PTY lifecycle, window and IPC code. It depends on `wardian-core`.

**`wardian-cli`** is a thin binary. It depends on `wardian-core`, uses `clap` for argument parsing, `serde_json` for output, and nothing Tauri-related.

The refactor is mechanical: move files, adjust `use` paths, update `Cargo.toml` manifests. No behavior changes in the GUI. It is a prerequisite for the rest of this spec.

### Component 2 — Binary, Distribution, Install

**Binary names:** the implementation binary is `wardian-cli` / `wardian-cli.exe` so it can coexist with the desktop app's `Wardian.exe` on Windows. The user-facing command remains `wardian`: macOS/Linux install a `wardian` shell launcher, and Windows installs `wardian.cmd`, both delegating to the adjacent `wardian-cli` binary.

**Build outputs:**

- Bundled inside the Tauri app under `resources/bin/wardian-cli[.exe]` via `src-tauri/tauri.conf.json` -> `bundle.resources`. [Release System](./2026-04-19-release-system.md) already reserves this slot.
- Per-platform release assets on GitHub Releases: `wardian-cli-{arch}-{os}[.exe]`. The [Release System](./2026-04-19-release-system.md) stubbed CLI matrix job is enabled as part of this spec.

**Install location:** `~/.wardian/bin/wardian` on macOS/Linux, `%USERPROFILE%\.wardian\bin\wardian.cmd` on Windows, with the implementation binary beside it. Per-user, no elevation required, inside the Wardian-owned tree alongside `state.db`, `agents/`, `classes/`.

**Install-time behavior:**

1. On first GUI launch or bundled CLI change, the Tauri app copies the implementation binary from `resources/bin/` into `~/.wardian/bin/`, writes the `wardian` launcher, and prepends the install directory to the user's `PATH`:
   - **Windows:** write to `HKCU\Environment\Path` and broadcast `WM_SETTINGCHANGE`.
   - **Unix:** append an `export PATH=<actual-wardian-bin>:"$PATH"` line to the user's shell profile (`.zshrc`, `.bashrc`, guarded by a `# wardian-cli` marker to stay idempotent).
2. If either step fails (read-only FS, no profile file, etc.), the GUI surfaces a dismissible notification with a copy-pasteable fallback command. The app continues to function; only the CLI is unreachable from user shells.
3. The binary inside the app bundle is a *source* artifact. The live binary is whatever is in `~/.wardian/bin/`. This separation is deliberate: the forthcoming updater swaps the live binary, not the bundled one, so GUI and CLI update paths decouple cleanly.

**Agent subterminal guarantee:** because `~/.wardian/bin` is on the user's real `PATH`, any shell an agent spawns — including nested shells, tmux panes, and provider-invoked subprocesses — finds `wardian` without needing environment injection. The CLI is reliably available, not dependent on the parent process propagating env correctly.

### Component 3 — State Access Model

The CLI first tries the running desktop app's local control endpoint for the same `WARDIAN_HOME`. This endpoint is intentionally narrow: it returns live read-only agent snapshots so status reflects the Rust backend's in-memory source of truth while the app is running.

If no desktop app is available for that home, the CLI opens `~/.wardian/state.db` directly, read-only, in WAL mode. This keeps introspection useful when the app is closed, and SQLite's WAL mode makes concurrent reads safe alongside the GUI's writes.

The live control endpoint is not Tauri command IPC. It is a local OS endpoint keyed by `WARDIAN_HOME` so dev, e2e, and production homes do not collide. Mutating commands still require a later control-plane spec, because commands like `wardian agent send` must push input into a live PTY and need stronger request semantics.

### Component 4 — Self-Resolution via `WARDIAN_SESSION_ID`

[Agent Identity and Status Tracking](./2026-04-20-agent-identity-and-status-tracking.md) injects `WARDIAN_SESSION_ID=<uuid>` into the environment of every spawned agent. The CLI uses this as its "who am I?" signal:

1. `wardian agent` (no args) reads `$WARDIAN_SESSION_ID`.
2. If set, looks up the matching row in `state.db` and returns it.
3. If unset, returns error code `not_in_session` with a hint suggesting `wardian agent <name>` for explicit lookup.

Because env vars propagate to subprocesses, this works transparently inside nested shells, subagents, and provider-spawned children — they all report the identity of the parent agent, which is the correct behavior (there is only one Wardian-managed agent in that process tree; the children are just its workers).

### Component 5 — Command Surface

```
wardian agent                          # self-lookup (shorthand for `agent show` with no args)
wardian agent <name-or-uuid>           # peer lookup (shorthand for `agent show <name>`)
wardian agent show [name-or-uuid]      # explicit form of the above
wardian agent list [filters]           # roster
```

**`show` filters / output modifiers** (apply to self and peer lookups):

- `--fields=a,b,c` — explicit field selection. Overrides the default set.
- `--field <name>` — return a single field's bare value (no JSON wrapper). For shell-script consumers.
- `--verbose` — include additional fields (`pid`, `started_at`, `workspace`, `last_status_at`).
- `--pretty` — human-readable block instead of JSON. Colorized when stdout is a TTY, plain when piped.

**`list` filters:**

- `--scope=workspace|all` — default `workspace` when the caller is itself a Wardian agent and its row has a workspace, default `all` otherwise. The `--scope` flag is a forward-compatible hook: future values (`team`, `graph:<id>`) land under the same flag without breaking existing scripts.
- `--status=<status>` — filter by `last_status` column (`idle`, `processing`, `action_required`, `error`, `off`, `headless`).
- `--class=<class-name>` — filter by agent class (`Architect`, `Coder`, …).
- `--workspace=<absolute-path>` — exact workspace filter; implies `--scope=all`.

### Component 6 — Output: JSON-First

The CLI's primary consumer is an agent, not a human. All non-`--pretty` output is JSON on stdout. JSON envelopes are indented by default so terminal output remains readable without piping through a formatter.

**Self / peer show envelope:**

```json
{
  "schema": 1,
  "agent": {
    "name": "coder-a1b2",
    "uuid": "7f3e…c19d",
    "class": "Coder",
    "provider": "claude-code",
    "workspace": "/path/to/workspace",
    "status": "processing"
  }
}
```

**List envelope:**

```json
{
  "schema": 1,
  "agents": [ { /* same shape */ }, … ]
}
```

**Default field set:** `name`, `uuid`, `class`, `provider`, `workspace`, `status`. Deliberately small: these are what an agent almost always needs, the shape is stable, and the response parses fast. `status_source` is available through `--fields` or `--field` when callers need to distinguish `live` snapshots from `persisted` DB fallback.

**`--verbose` adds:** `pid`, `started_at` (ISO 8601), `last_status_at` (ISO 8601).

**`--fields=…`:** explicit whitelist, replaces the default set entirely while preserving indented JSON output. Unknown field names error with code `invalid_field`.

**`--field <name>`:** emits the bare value followed by `\n`. No JSON wrapper, no schema envelope. Errors still go to stderr as JSON. This is the escape hatch for shell loops that don't want to shell out to `jq`.

**`--pretty`:** human block with one field per line, aligned. Colorizes status (emerald/cyan/amber/gray/red per the Wardian status palette) when stdout is a TTY; strips color otherwise.

**Schema versioning:** the `"schema": 1` field is load-bearing. Any breaking change to field semantics bumps the version. Additive changes (new optional fields under `--verbose`) do not. Agents that want to be strict can check the version before parsing.

**Sensitive data:** the `env` block of an agent row is never emitted by default. A future `--include-env` flag may expose it with an explicit opt-in; for this spec, env is not a field. Everything else in the row is considered shareable between agents.

### Component 7 — Errors

Errors are emitted as JSON on stderr. Stdout is empty on error, so scripts can rely on exit codes and `test -z "$(wardian agent … 2>/dev/null)"` semantics.

**Envelope:**

```json
{
  "schema": 1,
  "error": {
    "code": "not_in_session",
    "message": "WARDIAN_SESSION_ID environment variable is not set",
    "hint": "This command must run inside a Wardian-managed agent process. To look up a specific agent from outside one, pass a name or uuid: `wardian agent <name>`.",
    "details": { "command": "agent", "requested": "self" }
  }
}
```

- `code` — stable string identifier for programmatic matching. Agents branch on this.
- `message` — one-line human description. Never parse.
- `hint` — actionable next step written for an agent reader. Present whenever recovery is possible. This is what makes the CLI self-teaching.
- `details` — structured context, schema-free per error code, documented in the spec per code.

**Exit codes:**

| Code | Name              | Meaning |
| ---- | ----------------- | ------- |
| 0    | success           | Command completed, output on stdout. |
| 1    | generic           | Malformed arguments, internal error. |
| 2    | not_found         | Peer lookup returned no match. |
| 3    | not_in_session    | `WARDIAN_SESSION_ID` missing when self-lookup requested. |
| 4    | db_unavailable    | `state.db` missing, locked unrecoverably, or schema mismatch. |
| 5    | ambiguous         | Reserved. Cannot occur today since names are unique (spec 023). |

### Component 8 — Testing

- **Unit tests in `wardian-core`:** DB query functions (self-resolution, name-or-uuid resolution, list with filters), path-constant correctness.
- **Unit tests in `wardian-cli`:** argument parsing (all subcommand shapes), JSON output shaping for `show` / `list` / `--field` / `--pretty`, error envelope shaping, exit-code mapping.
- **Integration test:** seed a temp `state.db` via `wardian-core` helpers, spawn the binary with `WARDIAN_SESSION_ID` set to a seeded UUID, assert stdout JSON matches expected shape and exit code is 0. Runs under `cargo test` on all platforms; no special harness.
- **Native E2E coverage:** the native harness includes a shared-state test that creates agents through the running Tauri app and reads them back through the CLI using the same isolated `WARDIAN_HOME`.
- **`WARDIAN_HOME` isolation:** all tests honor the existing `WARDIAN_HOME` env var so they run against throwaway state directories.

### File Changes Introduced

**New:**

- `Cargo.toml` (workspace manifest at repo root).
- `crates/wardian-core/` with `Cargo.toml`, `src/lib.rs`, and the extracted modules.
- `crates/wardian-cli/` with `Cargo.toml`, `src/main.rs`, and submodules for command handlers and output formatting.
- `docs/specs/2026-04-21-wardian-cli-and-agent-command.md` (this document).

**Modified:**

- `src-tauri/`: imports updated to use `wardian-core`.
- `src-tauri/tauri.conf.json`: `bundle.resources` includes `resources/bin/wardian-cli[.exe]`.
- `.github/workflows/release.yml`: the stubbed CLI matrix job is uncommented and wired up to upload `wardian-cli-{arch}-{os}[.exe]` assets.
- Tauri first-launch logic (in `wardian-app`): install-and-PATH routine for `~/.wardian/bin/wardian`.

### Testing & Rollout Plan

1. **Workspace refactor merges first, behavior-neutral.** Verify GUI still builds, runs, and passes existing `cargo test` + `npm run test:e2e:native` suites.
2. **CLI crate lands with `wardian agent show` for self-lookup only.** Ship behind no flag — the binary exists but isn't bundled yet. Unit + integration tests green.
3. **Peer lookup and `list` land next.** Scope flag present but accepts only `workspace` / `all`.
4. **Tauri resource bundling and first-launch install.** Validate on all three OSes via the `workflow_dispatch` release dry-run from spec 021.
5. **Enable the CLI release-asset matrix job.** Cut a patch release to validate the full path from tag → bundled GUI with embedded CLI → standalone CLI assets on GitHub Releases.

**Rollback:** the workspace refactor is the only irreversible piece; everything else is additive and can be ripped out by deleting `crates/wardian-cli/` and reverting the Tauri first-launch hook. The workspace refactor is preserved regardless, since it has value independent of the CLI.

## Consequences

- **Positive:** agents gain a stable, textual introspection surface that works whether or not the GUI is running, and whether or not they are inside a Wardian-managed process tree.
- **Positive:** `wardian-core` eliminates the drift risk between GUI and CLI DTOs by construction — the same Rust types serialize both sides.
- **Positive:** the `wardian agent` namespace scales naturally to `spawn`, `send`, `kill`, `logs` without renaming or restructuring, and the `wardian <noun>` pattern generalizes to `workflow`, `library`, and global utilities.
- **Positive:** structured errors with `hint` fields make the CLI self-teaching. Agents that misuse it get pointed at the right usage instead of silently failing.
- **Positive:** per-user install with no elevation means the forthcoming updater can replace the binary without admin rights.
- **Negative:** the workspace refactor touches every Rust import path and is a disruptive one-time cost. Mitigated by landing it as an isolated, behavior-neutral commit before any CLI code.
- **Negative:** JSON-first output is less friendly to humans exploring the CLI interactively. Mitigated by `--pretty` existing from day one, and by the fact that humans are not the intended primary audience.
- **Negative:** modifying the user's shell profile / Windows PATH on first GUI launch is a side effect some users will dislike. Mitigated by a clear GUI notification when the change happens and a documented uninstall step.
- **Negative:** coupling the CLI's live binary location to `~/.wardian/bin` rather than the app bundle adds a copy step on first launch and means the CLI can drift from the GUI's version if the updater is interrupted. Resolved by the forthcoming auto-update spec treating the two binaries as one logical unit.
