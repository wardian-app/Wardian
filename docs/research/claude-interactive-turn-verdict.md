# Claude interactive turn verdict

Capability record for [#1492](https://github.com/wardian-app/Wardian/issues/1492).
It asks whether Claude Code's supported interactive surface can prove that a
prompt ended with a successful final answer, and it identifies the smallest
supported integrations that can.

- Reviewed: 2026-10-10
- Claude Code 2.1.296. The native Windows x64 executable's SHA-256 is
  `76de74f7694a5bdc03378a5f837c9e132827bf004c98c276bc4e9094ff06769a`.
- Evidence comes from the official documentation (links below) and read-only
  inspection of the JavaScript bundled in that executable. Minified
  identifiers change between builds, so citations name emitted strings and
  behavior instead.

## Finding

The interactive surface exposes no supported turn-end verdict.

| Surface | What it proves | Why it is not a verdict |
| --- | --- | --- |
| [`Stop` hook](https://code.claude.com/docs/en/hooks) | `prompt_id`, `last_assistant_message`, `stop_hook_active` | It runs before the continuation decision. Matching Stop hooks run in parallel, and any one of them may block or add context. |
| `StopFailure` hook | The turn ended on an API error. | It reports failures only, so its absence proves nothing. |
| OTel `claude_code.hook_execution_complete` ([monitoring](https://code.claude.com/docs/en/monitoring-usage)) | All Stop hooks of a round settled, with `num_blocking`, `additional_context_chars`, `num_cancelled`, `prompt.id`, and `event.sequence`. | See below. |
| Transcript `stop_hook_summary`, `turn_duration` | Hook summary and turn timing | The bundled SDK schema marks both `@internal`. `turn_duration` is written whether or not the turn succeeded. |
| Notification `idle_prompt` | Claude finished responding about 60 seconds ago. | It is a timing heuristic. |

After the Stop hooks settle, the handler for the `blockable_turn_end` hook
event can still change the outcome:

- It can continue the turn with no hook blocking: brief-mode enforcement, a
  deferred goal check-in, or structured-output enforcement. Each injects a
  continuation message.
- A hook can end the turn through `continue: false`, which yields the loop
  reason `stop_hook_prevented`. `hook_execution_complete` has no attribute
  for that decision.
- When the consecutive Stop-block cap overrides a block, the turn ends as
  `completed` even though `num_blocking` is non-zero.

Claude logs no OTel event after the turn-end decision. The bundle's complete
event list is: `api_error`, `api_refusal`, `api_request`,
`api_retries_exhausted`, `assistant_response`, `at_mention`, `auth`,
`compaction`, `feedback_survey`, `hook_execution_complete`,
`hook_execution_start`, `hook_plugin_metrics`, `hook_registered`,
`internal_error`, `managed_settings_resolved`, `mcp_server_connection`,
`permission_mode_changed`, `plugin_installed`, `plugin_loaded`,
`retention_sweep`, `skill_activated`, `subagent_completed`, `system_prompt`,
`tool`, `tool_decision`, `tool_result`, and `user_prompt`.

Settled Stop hooks are therefore a necessary condition for a final answer, but
never a verdict.

## Where the verdict exists

The query loop computes a terminal reason for every turn: `completed`,
`stop_hook_prevented`, `hook_stopped`, `max_turns`, `aborted_streaming`,
`aborted_tools`, `api_error`, and others. Only two surfaces expose it.

### SDK and `-p` result message

The [Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview) and
`claude -p --output-format stream-json` emit exactly one `result` message per
turn, after the turn's other messages. It carries `subtype`, `is_error`,
`terminal_reason`, `result` (the final assistant text, or the error text),
`session_id`, and `user_message_uuid`.

| Requirement | Assessment |
| --- | --- |
| Verdict | Supplied: `subtype`, `is_error`, `terminal_reason`. |
| Prompt identity | `user_message_uuid`, which the integration sets on each stream-json input message. |
| Response identity | `result` is the exact final text. |
| Native TUI | **Lost.** The session is driven programmatically, so Wardian would render the transcript instead of Claude's terminal UI. |
| Session continuity | The same conversation store supports `--resume`. |
| Authentication | The CLI's existing login. No new credential is needed. |
| Permissions | Must move to the SDK permission protocol (`canUseTool` or a permission prompt tool) instead of the TUI dialog. |
| Recovery | One result per turn, including API errors and interruptions (`aborted_*`). |

### Remote Control bridge

[Remote Control](https://code.claude.com/docs/en/remote-control) keeps the
local interactive TUI and relays the session through Anthropic's servers to
claude.ai and the mobile apps. The bridge emits a synthetic result for each
turn.

| Requirement | Assessment |
| --- | --- |
| Verdict | Present inside the bridge (`subtype`, `is_error`, `terminal_reason`). |
| Third-party access | **None documented.** Only claude.ai, the Claude apps, and Claude Desktop consume it. |
| Response identity | The synthetic success result sends an empty `result`; the final text is not included. |
| Authentication | A claude.ai Pro, Max, Team, or Enterprise login. API keys are not supported, and Team and Enterprise need an admin toggle. |
| Data handling | The transcript is stored on Anthropic servers. Zero Data Retention and HIPAA organizations cannot enable it. |
| Native TUI | Preserved. |

Remote Control is not a supported integration point for Wardian.

### Excluded internal paths

These exist in the bundle but are undocumented, so they are not used:

- the `CLAUDE_CODE_TEE_SDK_STDOUT` remote-runner I/O tee;
- the `@internal` transcript records listed above.

## Smallest supported options

1. **Provider change: a post-decision interactive signal.** Either a hook event
   or an OTel event emitted after the turn-end decision, carrying `prompt_id`,
   `terminal_reason`, `is_error`, and the final assistant message or its
   transcript UUID. This is the smallest change and keeps the native TUI. A
   `Stop` field such as `final: true` would not be enough, because the
   handler's decision follows the hooks.
2. **Wardian change: an SDK or stream-json Claude engine** for agents that need
   verified completions. It is supported today, but it replaces the native TUI
   and the permission dialog.

Until one of these exists, Wardian creates no Inbox completion for
interactive Claude turns. The Stop-round settlement evidence remains
available as the necessary condition that a future verdict must also satisfy.
