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

The CLI read-only `team` and `watchlist` commands read `watchlists/index.json` directly. They normalize the current v2 state shape and legacy flat watchlist arrays but do not use a separate persistence format.

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

## Git (`commands/git.rs`)

- `git_status`
- `git_current_branch`
- `git_log`
- `git_diff_file`
- `git_stage`
- `git_unstage`
- `git_discard_changes`
- `git_commit`
- `git_pull`
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

For payload semantics, see [IPC and Event Governance](./ipc-events.md) and the workflow engine docs.

## Change Management Guidance

When adding or changing commands:

1. Register/unregister in `src-tauri/src/lib.rs`.
2. Update frontend invoke callsites.
3. Update this reference page.
4. Update user or developer guides impacted by behavior changes.
