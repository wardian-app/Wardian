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
- `disable_agent_worktree`

Worktree commands update agent config only. UI callers that move an agent between the source checkout and a worktree must follow the config command with `clear_agent_session` so the provider starts fresh in the new workspace instead of resuming across a cwd change.
`enable_agent_worktree` accepts an optional `worktree_name`; when present, Wardian uses it for the `wardian/<slug>` branch and the per-agent `worktrees/<slug>` folder.

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

`submit_prompt_to_agent` is the provider-aware prompt path used when Wardian submits text into a live runtime. It should respect provider input readiness and delivery transaction results. Fixed sleeps before injection are not a correctness mechanism; delivery should wait for readiness evidence or queue the interaction.

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
- `load_queue_items`
- `save_queue_items`
- `load_queue_preferences`
- `save_queue_preferences`
- `load_agent_interactions`
- `save_agent_interactions`

The CLI read-only `team` and `watchlist` commands read `watchlists/index.json` directly. They normalize the current v2 state shape and legacy flat watchlist arrays but do not use a separate persistence format.

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

## Workflows (`commands/workflow.rs`)

- `list_workflows`
- `save_workflow`
- `delete_workflow`
- `run_workflow`
- `stop_all_triggers`
- `stop_workflow_triggers`
- `stop_workflow_run`
- `run_scheduled_workflow_now`
- `pause_all_triggers`
- `resume_all_triggers`
- `load_workflow_library`
- `save_workflow_library`
- `list_scheduled_runs`
- `create_scheduled_run`
- `delete_scheduled_run`
- `toggle_scheduled_run`

## Library (`commands/library.rs`)

- `get_library_tree`
- `save_library_item`
- `update_library_metadata`
- `open_library_folder`
- `deploy_skill`
- `remove_deployed_skill`
- `list_deployed_skills`
- `list_deployed_skill_refs`
- `list_skill_deployments`
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

- `git_status`: returns branch, upstream, ahead/behind counts, and file status entries.
- `git_current_branch`
- `git_log`: returns decorated commit graph rows with parent hashes, refs, author metadata, and ISO dates.
- `git_diff_file`
- `git_stage`
- `git_unstage`
- `git_discard_changes`
- `git_commit`
- `git_pull`
- `git_push`: pushes tracked branches and publishes untracked local branches with `--set-upstream`.
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
