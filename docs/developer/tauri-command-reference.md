# Tauri Command Reference

This page documents the current command surface registered in `src-tauri/src/lib.rs` and consumed by the React frontend.

## Naming and Payload Conventions

- Rust command names use `snake_case`.
- Frontend `invoke` calls use the same command names.
- Argument key casing depends on the command signature:
  - direct function arguments typically use Rust-style names
  - request structs can opt into `camelCase` via serde attributes (for example `SpawnAgentRequest`)

## Command Groups

## Agents (`commands/agent.rs`)

- `spawn_agent`
- `list_agents`
- `list_agent_metrics`
- `kill_agent`
- `pause_agent`
- `resume_agent`
- `clear_agent_session`
- `rename_agent`
- `reorder_agents`
- `update_agent_config`
- `enable_agent_worktree`
- `list_agent_worktrees`
- `assign_agent_worktree`
- `delete_agent_worktree`
- `disable_agent_worktree`

Worktree commands update agent config only. UI callers that move an agent between the source checkout and a worktree must follow the config command with `clear_agent_session` so the provider starts fresh in the new workspace instead of resuming across a cwd change.
`enable_agent_worktree` accepts an optional `worktree_name`. Wardian uses the provided name, or the agent session name when it is omitted, for the `wardian/<slug>` branch and the project-sibling `<source-checkout>.wt/<slug>` folder. `list_agent_worktrees` includes external Git worktrees for known source workspaces and marks only unassigned Wardian-managed worktrees as deletable. `delete_agent_worktree` removes only unassigned Git-registered worktrees under Wardian-managed roots, including legacy `<wardian-home>/agents/<session-id>/worktrees/` paths, cleans Wardian-generated build-cache redirects, and uses non-force Git removal; it does not remove assigned worktrees, source checkouts, or external worktree paths.

The live control protocol exposes CLI wrappers for the same worktree operations: `agent_worktree_list`, `agent_worktree_enable`, `agent_worktree_join`, and `agent_worktree_disable`. Unlike the raw Tauri worktree commands, the control mutation handlers call `clear_agent_session` after moving an agent workspace so CLI behavior matches the GUI flow.

## Terminal (`commands/terminal.rs`)

- `send_input_to_agent`
- `submit_prompt_to_agent`
- `send_binary_input_to_agent`
- `inject_session_input`
- `broadcast_input`
- `resize_agent_terminal`
- `read_agent_pty`

`send_input_to_agent` and `send_binary_input_to_agent` are raw PTY input paths for terminal interaction. They must not be used as the source of truth for structured agent-to-agent communication.

`submit_prompt_to_agent` is the structured prompt submission command for live
agents. It returns a `DeliveryDetail` and routes through the same backend
live-surface delivery service used by CLI sends, command panel sends, remote
prompt sends, workflow live routing, and mailbox drain. It must not be replaced
with raw `send_input_to_agent` calls for prompt injection. Fixed sleeps before
injection are not a correctness mechanism; delivery should wait for readiness
evidence or queue the interaction.

`read_agent_pty` drains buffered terminal output after `agent-pty-output-ready`. It is display data and compatibility evidence only. Structured ask/reply completion, Queue evidence, and provider status transitions must not depend on replaying this text.

## Classes (`commands/class.rs`)

- `list_agent_classes`
- `create_agent_class`
- `delete_agent_class`
- `get_default_class_instruction`
- `reset_class_to_default`
- `reset_all_class_prompts`

## Watchlists (`commands/watchlist.rs`)

- `load_watchlists`
- `save_watchlists`
- `load_watchlist_prefs`
- `save_watchlist_prefs`
- `load_queue_items`
- `save_queue_items`
- `load_queue_preferences`
- `save_queue_preferences`
- `load_agent_interactions`
- `save_agent_interactions`
- `load_opencode_last_assistant_text`

The CLI `team` and `watchlist` commands read and write `watchlists/index.json` directly. They normalize the current v2 state shape and legacy flat watchlist arrays for reads, write canonical v2 JSON for mutations, and best-effort notify the running app when the local control endpoint is available. Team create/add/split operations also seed communication-topology edges while preserving existing seed-suppression tombstones.

Queue commands persist the frontend Queue projection and preferences for the active Wardian home. Queue items should carry stable `evidence_id` and `evidence_source` fields when they are derived from provider runtime events, interaction-store events, or other live runtime evidence. Startup hydration may restore these items, but it must not create new completion or action-needed evidence.

`load_agent_interactions` and `save_agent_interactions` preserve the existing lightweight graph interaction projection. They are separate from the backend interaction control plane records used by structured `ask` and `reply`.

## Filesystem and Explorer (`commands/fs.rs`)

- `resolve_system_include_directories`
- `validate_directory_path`
- `get_explorer_root`
- `get_directory_tree`
- `delete_file`
- `reveal_in_explorer`
- `read_file_preview`

## Files resources (`commands/files.rs`)

These commands back Explorer-opened Files tabs. The frontend passes one
snake-case `request` object. Paths below are placeholders; callers must supply
an absolute local path.

### `open_file_resource`

Use one authorization mode: an `agent_id`, an exact live
`user_file_capability_id`, or neither for backend-trusted Workbench restore.
Supplying both is invalid. Trusted restore scans current agent primary and
additional roots in deterministic agent order, then exact live picker grants;
it never trusts persisted frontend state.

The returned subscription retains the exact requested pathname and its own
authorization claim even when another pathname resolves to the same canonical
`resource_id`. Reads and tickets revalidate that subscription-local provenance.
An alias subscription therefore fails after its symlink or junction is removed
or retargeted, while a direct subscription to the same resource remains valid.
All subscriptions still share one canonical watcher and revision stream.

Request:

```json
{
  "request": {
    "path": "<absolute-workspace-path>/README.md",
    "agent_id": "agent-session-id",
    "user_file_capability_id": null
  }
}
```

Response:

```json
{
  "resource_id": "file:<canonical-path>",
  "subscription_id": "subscription-uuid",
  "revision": 1,
  "descriptor": {
    "schema": 1,
    "canonical_path": "<absolute-workspace-path>/README.md",
    "display_name": "README.md",
    "extension": "md",
    "mime_type": "text/markdown",
    "encoding": "utf-8",
    "renderer_kind": "markdown",
    "size_bytes": 2048,
    "line_count": 42,
    "content_hash": "sha256:<content-hash>",
    "modified_at_ms": 1784242800000,
    "capabilities": {
      "preview": true,
      "changes": true,
      "draft": true,
      "stream": false
    },
    "unavailable_reason": null
  }
}
```

`content_hash` is a revision-identity field with an algorithm prefix. A
`sha256:` value is the exact digest of content scanned completely within its
renderer limits. A `bounded-sha256:` value is deliberately not a full-content
hash: it fingerprints the retained file identity, stable size/write metadata,
and a bounded leading detection probe. Wardian emits the bounded form for every
metadata-only scan that crosses its detected renderer ceiling. That includes
supported text/image/PDF resources disabled by byte or decoded image-pixel
limits and oversized unsupported/binary fallbacks that cross the text scan
ceiling. This lets watcher refreshes compare stable oversized revisions without
reading the entire file or mislabeling a partial digest as a content SHA.

Files above the 16 MiB Monaco limit, 64 MiB encoded/64-million-pixel image
limits, or 256 MiB PDF limit still return a successful metadata snapshot. Their
capabilities are all false and `unavailable_reason` is respectively
`monaco_size_limit_exceeded`, `image_limit_exceeded`, or
`pdf_size_limit_exceeded`. Text reads and stream-ticket issuance reject that
revision with the same typed reason; the bounded revision token never grants
content access.

### `read_file_resource_text`

Reads complete validated UTF-8 text only when the subscription and revision are
still current.

```json
{
  "request": {
    "resource_id": "file:<canonical-path>",
    "subscription_id": "subscription-uuid",
    "revision": 1
  }
}
```

```json
{
  "schema": 1,
  "resource_id": "file:<canonical-path>",
  "revision": 1,
  "text": "# Project\n"
}
```

### `save_file_resource_text`

Saves complete UTF-8 text through the exact live subscription. The revision and
hash are optimistic-concurrency inputs; the backend-private retained-handle
revision token remains the write authority and is never serialized.

```json
{
  "request": {
    "resource_id": "file:<canonical-path>",
    "subscription_id": "subscription-uuid",
    "expected_revision": 1,
    "buffer_base_hash": "sha256:<content-hash>",
    "text": "# Updated project\n",
    "recovery_cleanup": {
      "recovery_id": "recovery-uuid",
      "expected_recovery_revision": 4
    }
  }
}
```

The tagged response is `saved`, `unchanged`, or `stale_conflict`:

```json
{
  "status": "saved",
  "revision": 2,
  "content_hash": "sha256:<updated-content-hash>"
}
```

A stale conflict returns current metadata only after revalidating the
subscription. It does not return current file bytes.

`recovery_cleanup` is optional. The command derives recovery scope from the
calling WebView rather than request JSON. After `saved` or `unchanged`, it
best-effort removes only that exact recovery generation when its resource,
WebView, and CAS revision still match. A cleanup race leaves recovery intact
and does not change a committed save into an error.

### `checkpoint_file_recovery`

Creates or updates a durable dirty editor buffer through an exact live file
subscription. Create uses `null` for both recovery fields. Update supplies the
returned ID and exact current recovery revision.

```json
{
  "request": {
    "recovery_id": null,
    "expected_recovery_revision": null,
    "resource_id": "file:<canonical-path>",
    "subscription_id": "subscription-uuid",
    "base_revision": 1,
    "base_content_hash": "sha256:<base-content-hash>",
    "resource_key": "file:<canonical-path>",
    "buffer": "# Unsaved edit\n"
  }
}
```

The backend derives WebView scope from the Tauri caller. The returned metadata
contains `recovery_id`, `recovery_revision`, base hash/opaque revision, and
timestamps; the private retained file revision never crosses IPC.

### `get_file_recovery`

Returns only the stored base and editor buffer for the exact recovery ID,
stable resource key, and calling WebView. It intentionally works after restart
when current file authorization is unavailable.

```json
{
  "request": {
    "recovery_id": "recovery-uuid",
    "resource_key": "file:<canonical-path>"
  }
}
```

This command cannot read current disk bytes, save, or recreate an expired or
revoked file capability.

### `discard_file_recovery`

Removes one exact scoped recovery generation after a recovery CAS check.

```json
{
  "request": {
    "recovery_id": "recovery-uuid",
    "expected_recovery_revision": 4,
    "resource_key": "file:<canonical-path>"
  }
}
```

### `merge_file_recovery`

Requires a newly verified live subscription for the same target, reads the
current authorized UTF-8 disk head, and runs a three-way merge against the
stored base and buffer.

```json
{
  "request": {
    "recovery_id": "recovery-uuid",
    "expected_recovery_revision": 4,
    "resource_key": "file:<canonical-path>",
    "resource_id": "file:<canonical-path>",
    "subscription_id": "new-live-subscription-uuid"
  }
}
```

The tagged response is `clean` or `conflicted`. Both contain
`recovery_revision`, `current_revision`, `current_content_hash`,
`disk_changed`, and `merged_text`. Conflict text includes explicit markers and
both sides. The command never writes the file.

### `pick_file_resource_save_target`

Opens the native save dialog and returns a short-lived one-shot capability for
the exact selected parent identity and basename. Canceling returns `null`.

```json
{
  "request": {
    "title": "Save As",
    "default_name": "README-copy.md"
  }
}
```

```json
{
  "schema": 1,
  "save_target_grant_id": "save-target-uuid",
  "selected_path": "<absolute-workspace-path>/README-copy.md"
}
```

### `save_file_resource_as_text`

Consumes an exact native save-target grant and atomically creates or replaces
only that ordinary-file target. The command has no source-resource or artifact
field, so opening the returned file and closing the source are a separate
frontend transaction.

```json
{
  "request": {
    "save_target_grant_id": "save-target-uuid",
    "text": "# Saved copy\n"
  }
}
```

```json
{
  "schema": 1,
  "capability_id": "user-file-capability-uuid",
  "canonical_path": "<absolute-workspace-path>/README-copy.md",
  "resource_id": "file:<canonical-path>",
  "content_hash": "sha256:<saved-content-hash>"
}
```

### `issue_file_resource_ticket`

Mints a short-lived image/PDF stream ticket bound to the resource,
subscription, current revision, calling WebView, and caller-owned renderer
lease.

```json
{
  "request": {
    "resource_id": "file:<canonical-path>",
    "subscription_id": "subscription-uuid",
    "revision": 1,
    "renderer_lease_id": "files-pane-renderer-1"
  }
}
```

```json
{
  "schema": 1,
  "ticket_id": "ticket-uuid",
  "url": "wardian-resource://localhost/ticket-uuid",
  "resource_id": "file:<canonical-path>",
  "revision": 1,
  "renderer_lease_id": "files-pane-renderer-1",
  "expires_at_ms": 1784242860000
}
```

The URL accepts `GET` and `HEAD`, advertises `Accept-Ranges: bytes`, returns
`206` plus `Content-Range` for a valid single range, and returns `416` plus
`Content-Range: bytes */<size>` for an unsatisfiable range. Responses are
`no-store` and `nosniff`; a `HEAD` response never carries a body.

The returned URL is a backend protocol identifier, not a WebView-ready asset
URL. Frontend consumers pass it through the shared Files resource URL adapter,
which percent-decodes the ticket path and calls Tauri `convertFileSrc` once
with the `wardian-resource` protocol. Image and PDF renderers consume only that
converted client result; already usable HTTP(S), blob, data, and test URLs are
left unchanged rather than converted twice.

Coverage is intentionally layered: frontend tests assert the adapter call and
the exact URL consumed by both binary renderers, while native Files E2E reloads
a persisted Files image surface and waits for the real `ImageRenderer` to
decode bytes through the converted custom-protocol URL. The same native test
also probes `GET`, `HEAD`, and range responses directly.

Ticket issuance copies and verifies the requested revision into an immutable,
backend-owned snapshot before returning the URL. Later source edits, atomic
file replacement, or deletion do not change bytes served by that ticket. A
new revision requires a new ticket. Snapshot storage is bounded, and the
ticket deadline actively removes an abandoned snapshot and its matching
renderer lease. Expiry cleanup is issuance-aware, so an older timer cannot
revoke a newer ticket that reused the same renderer lease ID.

### `close_file_renderer_lease`

Revokes only the matching WebView renderer lease and every ticket it owns. The
file subscription remains open for the pane and other renderers.

```json
{
  "request": {
    "resource_id": "file:<canonical-path>",
    "subscription_id": "subscription-uuid",
    "renderer_lease_id": "files-pane-renderer-1"
  }
}
```

Response: `null`.

### `close_file_resource`

Releases one subscription. It is idempotent; the shared watcher is removed only
after the final subscription closes. Closing does not remove or replace another
subscription's authorization provenance.

```json
{
  "request": {
    "subscription_id": "subscription-uuid"
  }
}
```

Response: `null`.

### `pick_file_resource`

Opens the native picker and records a backend-owned grant for the exact selected
canonical file. The grant does not authorize a sibling or parent directory.
Picker grants are deduplicated by canonical target and bounded to 128 entries;
dormant least-recently-used grants are evicted before a new grant is rejected.
Deduplication does not widen an open subscription: each open retains the exact
picker-selected pathname it used, including an alias spelling.

```json
{
  "request": {
    "title": "Open a file"
  }
}
```

```json
{
  "schema": 1,
  "capability_id": "capability-uuid",
  "canonical_path": "<absolute-workspace-path>/report.pdf"
}
```

Cancel returns `null`. Resource-local failures use the same typed shape for all
Files commands:

```json
{
  "schema": 1,
  "code": "stale_revision",
  "message": "requested revision is no longer current"
}
```

Stable codes include `invalid_request`, `unauthorized_path`,
`unavailable_path`, `unstable_file`, `resource_not_found`, `stale_revision`,
`unsupported_content`, `file_too_large`, `monaco_size_limit_exceeded`,
`monaco_line_limit_exceeded`, `image_dimensions_unavailable`,
`image_limit_exceeded`, `pdf_size_limit_exceeded`, `grant_limit_reached`,
`ticket_capacity_exceeded`, `invalid_ticket`, `unauthorized_ticket`,
`expired_ticket`, `invalid_range`, and `range_not_satisfiable`.
`grant_limit_reached` means all 128 exact picker grants are currently active;
closing an unused file makes a grant evictable. `ticket_capacity_exceeded`
means immutable renderer snapshots have reached their bounded storage budget;
closing the owning renderer lease or waiting for active expiry releases it.

Debug builds additionally register `debug_grant_file_resource_for_e2e` and
`debug_file_resource_stats` for the native harness. The former delegates to the
same exact grant function as the native picker; the latter exposes aggregate
ownership counts only. Both are compiled out of release builds and are not
frontend application APIs.

## Workflows (`commands/workflow.rs`)

Current workflow commands:

- `workflow_parse`
- `workflow_validate`
- `workflow_write`
- `workflow_list_blueprints`
- `workflow_list_runs`
- `workflow_read_run`
- `workflow_run`
- `workflow_resume`
- `workflow_approve`
- `workflow_cancel`
- `schedule_create`
- `schedule_list`
- `schedule_pause`
- `schedule_resume`
- `schedule_remove`
- `schedule_run_now`

Old workflow system command names such as `run_workflow`, `list_workflows`,
`list_scheduled_runs`, and `create_scheduled_run` belong to the retired JSON
workflow system. Do not add new frontend behavior against those names.

## Library (`commands/library.rs`)

- `get_library_index`
- `read_library_item`
- `save_library_item`
- `update_library_metadata`
- `create_library_folder`
- `rename_library_entry`
- `delete_library_entry`
- `open_library_folder`
- `deploy_skill`
- `remove_deployed_skill`
- `list_deployed_skills`
- `list_deployed_skill_refs`
- `list_skill_deployments`
- `set_skill_deployments`
- `remove_orphan_deployment`
- `library_watch`
- `library_unwatch`

## Settings and Patch (`commands/settings.rs`, `commands/patch.rs`)

- `run_gemini_patch`
- `list_available_shells`
- `get_settings_folder_path`
- `load_shell_settings` returns resolved shell settings plus sparse user overrides.
- `save_shell_settings` accepts the versioned shell settings document and persists sparse overrides.
- `load_app_settings` returns resolved app settings plus sparse user overrides.
- `save_app_settings` accepts the versioned app settings document and persists sparse overrides.
- `save_agent_session_persistence`
- `save_opencode_theme`

## Remote Access (`commands/remote.rs`)

- `load_remote_access_status`
- `load_remote_gateway_config`
- `save_remote_gateway_config`
- `create_remote_pairing_offer`
- `list_remote_devices`
- `list_pending_remote_pairing_requests`
- `load_remote_setup_check`
- `approve_remote_pairing_request`
- `reject_remote_pairing_request`
- `revoke_remote_device`

Debug builds also register `debug_create_remote_session` for native gateway
tests. It is not registered in release builds.

## Git (`commands/git.rs`)

- `git_status`
- `git_init`
- `git_clone_repository`
- `git_current_branch`
- `git_log`
- `git_commit_changes`
- `git_diff_file`
- `git_diff_file_against_workspace`
- `git_show_file_revision`
- `git_stage`
- `git_unstage`
- `git_discard_changes`
- `git_ignore`
- `git_commit`
- `git_commit_signed`
- `git_commit_staged_signed`
- `git_commit_all_signed`
- `git_commit_signed_no_verify`
- `git_commit_staged_signed_no_verify`
- `git_commit_all_signed_no_verify`
- `git_commit_no_verify`
- `git_commit_staged_no_verify`
- `git_commit_all_no_verify`
- `git_commit_empty`
- `git_commit_empty_no_verify`
- `git_commit_amend`
- `git_commit_amend_no_verify`
- `git_commit_staged_amend_no_verify`
- `git_commit_all_amend_no_verify`
- `git_commit_staged_amend`
- `git_commit_all_amend`
- `git_undo_last_commit`
- `git_rebase_abort`
- `git_pull`
- `git_list_branches`
- `git_checkout_branch`
- `git_create_branch`
- `git_stash_push`
- `git_list_stashes`
- `git_show_stash`
- `git_stash_staged`
- `git_stash_apply_latest`
- `git_stash_apply`
- `git_stash_pop_latest`
- `git_stash_pop`
- `git_stash_drop`
- `git_stash_drop_all`
- `git_fetch`
- `git_push`
- `git_create_worktree`
- `git_remove_worktree`
- `git_watch`
- `git_unwatch`

## Event Surface (Backend -> Frontend)

Common app-level events:

- `agent-metrics`
- `agent-json-event`
- `agent-pty-output-ready`
- `agent-terminal-cleared`
- `agents-updated`
- `workflow-telemetry`
- `workflow-progress`
- `workflow-status-updated`
- `scheduled-runs-updated`
- `git-changed`
- `library-changed` with payload `{ "library_type": "skills" }`
- `file-resource://revision` with the next stable Files descriptor

For payload semantics, see [IPC and Event Governance](./ipc-events.md) and the workflow engine docs.

## Live Control Protocol

The standalone `wardian` CLI primarily talks to the desktop app through Wardian's live control endpoint rather than Tauri `invoke`. These command contracts share DTOs with `crates/wardian-core/src/control.rs`:

- `send_message`
- `ask`
- `submit_reply`
- `agent_watch`

`send_message` routes provider-aware delivery to one or more targets. Delivery responses contain `runtime_state`, `delivery_state`, `input_mode`, optional `message_id`, `delivery_phase`, `observed_state`, `reason`, `profile`, and provider-specific error details. When the target runtime is not ready for live input, the command should queue the message in the mailbox or fail according to its queue policy instead of injecting text early. This slice does not create durable message interactions for ordinary `send` calls; structured durability belongs to `ask` and `reply`.

`ask` creates a task interaction with `reply_required`, delivers the prompt plus reply instructions, and waits for the parent task to reach a terminal structured state. It returns the attached structured reply when complete. Output-marker waiting remains a compatibility mode, but it is not the structured ask/reply completion path.

`submit_reply` resolves the request ID against the interaction store, creates or attaches a reply interaction, and completes the parent task. Unknown request IDs fail deterministically. Duplicate replies must be rejected or handled by an explicit idempotency policy. When `origin` contains a Wardian agent session ID, the backend verifies that the sender is the task target.

`agent_watch` returns ordered status, transcript, output, and delivery evidence from watch state. `delivery` snapshots are derived from delivery watch events. Raw PTY output is opt-in and should be used only for terminal rendering or transport debugging.

Provider input readiness is tracked separately from provider install readiness. Live delivery gates on per-session provider input state:

```json
{
  "session_id": "uuid-1",
  "generation": 7,
  "state": "ready",
  "ready_evidence": "prompt_detected",
  "observed_at": "2026-05-25T16:00:00.000Z"
}
```

Provider runtime status remains authoritative for provider-internal states such as `action_required`. Interaction status tracks Wardian-owned delivery and reply lifecycle only.

## Change Management Guidance

When adding or changing commands:

1. Register/unregister in `src-tauri/src/lib.rs`.
2. Update frontend invoke callsites.
3. Update this reference page.
4. Update user or developer guides impacted by behavior changes.
