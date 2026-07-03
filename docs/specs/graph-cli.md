# Spec: Graph CLI (`wardian graph`)

- **Status**: Approved design (brainstormed 2026-07-03)
- **Issue**: [#632](https://github.com/wardian-app/Wardian/issues/632)
- **Related**: [#610](https://github.com/wardian-app/Wardian/issues/610) (communication topology control surface — the GraphView side, merged in PR #629), `docs/specs/communication-topology.md` (data model and resolver semantics this spec builds on)

## Strategic Rationale

The Graph view (#610) made the communication topology an editable control surface — for humans. Agents, however, live in the CLI. This spec adds the complementary CLI surface so agents can **observe and control the same topology**: inspect their neighbor set and why each member is visible, see live communication activity, wire themselves into collaborations, and formalize or dismiss unmapped (ghost) suggestions — all without the app running.

The design deliberately reuses every existing primitive: `topology.json` stays the single source of truth, `wardian_core::topology` provides all read/write and resolution logic, and interaction records in the shared SQLite DB provide activity. No new persistence formats.

### Core semantic: self-serve, not self-defense

Inside an agent session, mutations are restricted to edges **involving the calling agent**. This is a guard against agents silently rewiring *other* agents' visibility, not a security boundary — the topology remains a soft boundary (see #610 spec), and any process can edit `topology.json` directly. Outside a session, the caller is the human operator (or an orchestrator script run deliberately outside agent context) and arbitrary pairs are allowed.

## Command Surface

New top-level clap subcommand `Graph(GraphArgs)` in `crates/wardian-cli/src/args.rs`, handled by a new `crates/wardian-cli/src/graph.rs` module (keeps `main.rs` from growing further).

All commands support the CLI's standard output modifiers: JSON by default, `--pretty` for human tables, following `output.rs` conventions.

### Observation

| Command | Behavior |
|---|---|
| `wardian graph show` | Whole-graph snapshot: `agents` (uuid, name, status, workspace), `edges` (manual, with `created_at`), `unmapped_pairs` (recent traffic between unconnected agents), `ignored_pairs`. |
| `wardian graph neighbors [agent]` | Resolved neighbor view for one agent via `resolve_neighbors`: members with reasons (`manual`, `rule:workspace-fallback`). Defaults to self when `WARDIAN_SESSION_ID` is set; outside a session the agent argument is required (exit 1 with a hint otherwise). |
| `wardian graph activity` | Per-pair communication state from interaction records: `a`, `b`, `last_message_at`, `active_ask`, `awaiting_reply_from`, plus `unmapped: bool` (activity exists but no manual edge and pair not ignored). |

### Mutation

| Command | In a session | Outside a session |
|---|---|---|
| `wardian graph link <other> [<b>]` | One arg: me ↔ other. Two args: allowed only if one endpoint resolves to self. | Two args required: link any pair. |
| `wardian graph unlink <other> [<b>]` | Same identity rules as `link`. | Two args required. |
| `wardian graph ignore <other> [<b>]` | Same identity rules. Durable dismissal of an unmapped suggestion. | Two args required. |
| `wardian graph unignore <other> [<b>]` | Same identity rules. | Two args required. |

**Ghost normalization**: there is no separate "formalize"/"approve" verb. Linking an unmapped pair *is* formalization (identical write path); `ignore` is dismissal. Unmapped pairs surface only as flagged data in `show` and `activity` output.

**Idempotency**: mutations succeed (exit 0) whether or not they changed anything; output includes `"changed": true|false`. Re-running a wiring script never fails. Errors — unknown agent, self-link (`a == b`), permission violation, missing required argument — exit 1 with a message on stderr.

**Target resolution**: endpoints accept agent names or UUIDs via the CLI's existing agent-resolution logic. Both endpoints must resolve to known agents; edges to arbitrary unknown UUIDs are rejected (they would be garbage-collected by `retain_agents` anyway).

## Identity Resolution

- Self = the agent whose UUID is in `WARDIAN_SESSION_ID` (set inside Wardian agent terminals), consistent with `agent list --scope auto`.
- The permission check happens **after** target resolution: resolve both endpoints, then require one to equal self when in a session.

## Data Flow

All CLI logic reads and writes through `wardian_core`:

- **Structure**: `load_topology(home)` → mutate via `Topology::{add_edge, remove_edge, ignore_pair, unignore_pair}` → `save_topology(home, &topology)`. Save is already atomic (temp + rename) and canonicalizing.
- **Agents**: the same disk-state agent listing `agent list` uses, mapped to `AgentRef { uuid, workspace }` for the resolver.
- **Activity**: `db::list_interaction_records()` → `pair_activity_from_records(records, now_ms)` with `now_ms` = current UTC epoch millis. Same aggregation the app's `get_pair_activity` command uses.
- **Unmapped pairs**: pairs present in activity but absent from manual edges and not in `ignored_pairs` — the same derivation the GraphView performs.

## App-Side: topology.json File Watcher

Today the docs claim "the app reloads on changes" to `topology.json`, but no watcher exists — GraphView refreshes only on the `topology-changed` event, which only app-side commands emit. CLI writes (and hand edits) are invisible to an open GraphView until reload.

**Addition**: a `notify` file watcher on `<WARDIAN_HOME>/topology.json`, registered at app startup, following the existing watcher pattern in `commands/fs.rs` / `commands/git.rs`:

- Debounced (single emit per burst of filesystem events; the atomic rename produces multiple events on some platforms).
- Emits the existing `topology-changed` event — GraphView already listens; **no frontend changes required**.
- The app's own saves also trip the watcher. This is a harmless idempotent refetch: the GraphView's layout freeze means nodes don't move on refetch.
- Watcher construction and debounce logic are factored into a function taking an emit callback, so the logic is unit-testable without a Tauri `AppHandle`.

## Edge Cases

- **Missing/corrupt `topology.json`**: loads as empty topology (existing `load_topology` behavior). Observe commands succeed on a fresh home; a first `link` creates the file.
- **Write race (app vs CLI)**: both writers do atomic whole-file read-modify-write; concurrent writes are last-writer-wins and one edit can be lost. Accepted for v1: edits are human/agent-paced, edges are trivially re-creatable, and both sides converge on the next read. No file locking.
- **`WARDIAN_SESSION_ID` set to an unknown agent** (stale env, deleted agent): treat as no-session? No — fail closed: mutations exit 1 ("session agent not found"); `neighbors` with no argument likewise errors. Prevents a stale session silently acquiring operator powers.
- **Self-link** (`link X X` or resolving to the same UUID): exit 1, mirroring `canonical_pair` rejection.
- **`unlink` on a nonexistent edge / `ignore` on an already-ignored pair**: exit 0, `"changed": false`.

## Testing

Per the layered test plan (lowest layer that proves the behavior):

1. **CLI integration tests** (`crates/wardian-cli/tests/graph_cli.rs`, temp `WARDIAN_HOME`, pattern from `agent_cli.rs`):
   - Each command's happy path against seeded `topology.json` + agent state.
   - Session vs no-session permission paths (env var set/unset; single-threaded or env-guarded per repo convention).
   - Idempotency (`changed: false` cases), self-link rejection, unknown-agent rejection, stale-session fail-closed.
   - `show`/`activity` unmapped derivation against seeded interaction records.
2. **Core unit tests**: none expected — all core APIs exist and are covered; any new core helper (e.g. unmapped-pair derivation if shared) gets unit tests in `wardian-core`.
3. **Watcher unit test** (`cargo test` in `src-tauri`): debounce/emit logic via callback injection with a temp file.
4. **Native E2E** (optional, existing harness): CLI `link` while app runs → GraphView receives `topology-changed`. Only if the native harness is available; the watcher unit test plus existing GraphView event tests already cover both halves.

## Documentation

- New **Graph** section in `docs/guide/cli.md` (commands, permission model, examples in bash-first form per cross-platform doc rules).
- `docs/guide/graph.md`: cross-link the CLI surface; the "app reloads on changes" claim becomes true — keep and reference the watcher.
- This spec at `docs/specs/graph-cli.md`.

## Non-Goals

- `graph watch` / follow mode (streaming changes) — deferred, YAGNI for v1.
- File locking / multi-writer coordination for `topology.json`.
- Any change to resolver semantics, edge schema, or the soft-boundary model (all owned by the #610 spec).
- Frontend changes (GraphView already consumes `topology-changed`).

## Decision Log

- **Self-serve permissions** (2026-07-03): in-session mutations must involve self; no-session callers are unrestricted. Chosen over full read/write (agents could rewire others) and observe-only (blocks programmatic team wiring).
- **Namespace `wardian graph`** (2026-07-03): matches the UI surface name over `wardian topology` (file name) or extending `wardian agent`.
- **Ghost verbs normalized onto `link`/`ignore`** (2026-07-03): no `graph ghosts` noun or formalize verb; unmapped pairs are flagged rows in `show`/`activity`.
- **Direct file/DB access + app-side watcher** (2026-07-03): over routing mutations through the app (no IPC channel exists; CLI must work app-less) and over read-only v1.
- **Idempotent mutations with `changed` flag** (2026-07-03): agent-friendly; wiring scripts are safely re-runnable.
