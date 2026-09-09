# Provider Runtimes

Wardian provides one orchestration layer over six supported CLI providers: Antigravity, Claude Code, Codex, OpenCode, Pi, and Gemini CLI. Each provider keeps its native command-line behavior, while Wardian adapts session identity, working roots, skill discovery, status tracking, and automation execution into a consistent app model.

## Overview

| Provider | Support | Working Root | Instruction Source | Skill and Context Model | Session Identity |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **[Antigravity](https://www.antigravity.google/docs/cli-overview)** | Supported | Real target workspace | `AGENTS.md` | Wardian include roots passed through repeated `--add-dir` flags | Captured after the first real prompt |
| **[Claude Code](https://github.com/anthropics/claude-code)** | Supported | Real target workspace | `CLAUDE.md` | `--add-dir` instruction roots plus `.claude/skills` links to Wardian-managed skills | Wardian assigns fresh session IDs and resumes explicitly |
| **[Codex](https://github.com/openai/codex)** | Supported | Real target workspace via `--cd` | `AGENTS.md` | Per-agent `CODEX_HOME` habitat with scoped skill projection | Fresh local rollout, then exact resume |
| **[OpenCode](https://github.com/anomalyco/opencode)** | Supported | Real target workspace | `AGENTS.md` plus injected runtime config | `OPENCODE_CONFIG` adds Wardian instructions; `OPENCODE_CONFIG_DIR` exposes projected skills | Discovered from JSON events and resumed with `--session` |
| **[Pi](https://pi.dev/docs/latest/)** | Supported | Real target workspace | `AGENTS.md` | Wardian instruction files use `--append-system-prompt`; skill roots use repeated `--skill` | Wardian assigns an exact project session ID and resumes explicitly |
| **[Gemini CLI](https://github.com/google-gemini/gemini-cli)** | Unmaintained | Real target workspace | `GEMINI.md` | Wardian include roots passed through `--include-directories`; Gemini patch enables multi-root skill discovery | Discovered from provider output |

## Shared Runtime Model

- The Rust backend is the source of truth for process lifecycle, provider session IDs, PTY ownership, and status telemetry.
- Regular visible agents use the global session policy unless the agent has an explicit override.
- Automation Agent nodes choose one run mode: `ephemeral`, `inherit_fresh`, or `inherit_resume`.
- Wardian keeps user repositories clean by adapting provider-native discovery instead of copying agent-specific instruction and skill files into the project root.

## Model and Effort Selection

Choose a model when you spawn an agent or from **Agent Configuration** for an
existing agent. The same compact control is available in Chat. Wardian keeps
the selection on the agent and applies it the next time that provider starts or
restarts; changing it does not interrupt an active turn.

The picker reads the installed provider's available models instead of pinning a
dated list into Wardian. It refreshes automatically while open and has a manual
refresh action. If a provider cannot expose its current catalogue, Wardian
keeps a saved model rather than replacing it with a guess.

Where a provider exposes compatible reasoning levels, the picker also shows an
**Effort** control for the selected model. Providers without a stable
launch-time effort option show only model selection. Leave either control on
**Provider default** to use the provider's normal default.

Agents and scripts can inspect the same live catalogue through the CLI:

```bash
wardian agent models --provider <provider> --refresh
```

Use the provider default for routine, bounded work. For complex, ambiguous,
multi-step work such as architecture, deep debugging, or security review, an
orchestrator may choose a listed compatible model and higher available effort.
Do not guess model IDs or effort values, and do not infer a need for high
effort from an agent class alone. The CLI accepts selections at spawn or update;
updating an existing running agent requires a restart before the provider sees
the change.

## Antigravity (`agy`)

Antigravity runs directly in the real target workspace.

### Instruction and Skill Discovery

Antigravity reads `AGENTS.md`. Wardian passes common, class, and agent include roots with repeated `--add-dir <absolute-path>` flags so the CLI can load Wardian-managed context without copying agent files into the repository.

Wardian-managed roots usually live under hidden `.wardian` directories. Antigravity can ignore or under-discover hidden include roots, so Wardian exposes those roots through visible temp projections under the system temp directory before passing them to `agy`. Roots that contain `.agents/skills` are materialized into that projection instead of linked directly, because Antigravity does not reliably discover skills that are nested links back into hidden Wardian storage. Skill deploy/remove operations refresh live Antigravity projections, and the library skill watcher refreshes projections after skill-file changes while it is active. Restarting the agent rebuilds the projection from the canonical Wardian roots.

### Session and Status Handling

Wardian launches visible Antigravity agents with `agy --prompt-interactive ""` so the CLI starts in interactive mode without an initial task. Headless automation runs use `agy --print` and, when resuming, `--conversation <conversation-id>`. Provider options include `--sandbox`, `--dangerously-skip-permissions`, and `--print-timeout <duration>`.

**New Session** starts a fresh interactive Antigravity session without a hidden bootstrap prompt while retaining the Wardian agent, habitat, and saved history. Wardian stores the provider conversation ID after the first real user prompt updates Antigravity's workspace conversation mapping. If Wardian restarts before that first prompt, the agent starts fresh again because no provider conversation exists yet to resume.

Antigravity stores runtime state under `~/.gemini/antigravity-cli`. Wardian discovers the conversation ID from the provider cache and reads `brain/<conversation-id>/.system_generated/logs/transcript.jsonl` for status, assistant transcript text, and tool activity. `wardian agent watch` uses completed `MODEL` `PLANNER_RESPONSE` transcript records as provider-adapted assistant output, planner `tool_calls` as tool-call rows, and model action records such as `RUN_COMMAND`, `VIEW_FILE`, `CODE_ACTION`, `SEARCH_WEB`, `LIST_DIRECTORY`, `GREP_SEARCH`, `READ_URL_CONTENT`, `ASK_QUESTION`, and `GENERIC` as tool-result rows.

### Debug First

If Antigravity starts but Wardian does not show assistant text, inspect the conversation cache and transcript path above. If `agy --print` returns empty stdout, check the transcript before treating the run as failed.

## Claude Code (`@anthropic-ai/claude-code`)

Claude runs directly in the real target workspace.

### Instruction and Skill Discovery

Claude reads `CLAUDE.md`. Wardian enables additional-directory discovery and maintains `.claude/skills` links where needed so Claude can see Wardian-managed common, class, and agent skills without those files living in the repository root.

Keep shared, class, and agent instructions in their canonical `AGENTS.md` files.
On ordinary Claude bootstrap, Wardian refreshes its existing managed `CLAUDE.md`
bridges as sibling copies and generates the habitat copy after its memory brief
is appended. These are snapshots: start a fresh session after editing canonical
instructions to load the updated text. No separate preparation command is needed.

Generated copies carry an ownership marker and body hash. Wardian preserves
customized or linked `CLAUDE.md` files; editing a generated copy makes it a custom
override. To restore automatic refresh for an existing managed bridge, replace
that override deliberately with the bare `@AGENTS.md` stub. Workspace files and
user-selected include directories are not rewritten. Nested imports in canonical
text retain Claude's normal external-import consent; Wardian does not grant
project-wide approval or change global trust settings.

Wardian also launches Claude-managed terminal surfaces with Claude Code's alternate-screen opt-out enabled. This preserves native terminal scrollback for desktop terminals and mobile PWA drag scrolling while keeping Claude's existing `CLAUDE.md` discovery behavior.

### Session and Status Handling

Wardian assigns fresh Claude session IDs up front and uses explicit resume flags for resumed provider sessions. Claude permission requests are captured through a generated hook under the Wardian agent directory, which lets the UI surface `Action Needed` with request details.

Visible Claude agents run through Claude Code's interactive mode. Do not pass `--input-format stream-json` or `--output-format stream-json` to interactive launches; Claude Code treats those as print-mode flags. Wardian keeps stream-json output only for headless/bootstrap flows that also pass `--print`.

### Debug First

If Claude appears blocked, inspect the permission hook output, `CLAUDE.md` discovery, and resume flags before treating the issue as a generic PTY failure. If mobile or remote drag scrolling fails only for Claude, verify that the managed launch environment still includes Claude Code's alternate-screen opt-out.

## Codex (`@openai/codex`)

Codex executes against the real target workspace while Wardian keeps mutable provider state in an agent habitat.

### Instruction and Skill Discovery

Codex reads `AGENTS.md`. Wardian passes the real project root with `--cd <absolute-workspace-path>` and projects assigned skills into the agent-specific `CODEX_HOME/skills` tree. This keeps skill scope per agent while preserving Codex trust and command execution against the actual repository path.

Each agent keeps its own mutable Codex home. Wardian links that home's
`sessions/` directory to the native Codex `sessions/` directory, using a
Windows junction where supported and the platform equivalent elsewhere. Local
rollouts are migrated without changing their filenames; if linking fails, the
local sessions tree is restored and the provider continues in local-only mode.

The provider writes its local `history.jsonl` and `session_index.jsonl` files,
while Wardian alone atomically republishes complete, validated,
de-duplicated records to the native central copies under a cross-process lock.
Invalid central tails are repaired during that publication. Codex SQLite databases,
including WAL/SHM sidecars, remain isolated per agent. `auth.json` and
`cap_sid` are copied inward only and never projected back outward. This layout
was verified against Codex CLI `0.150.1` and should be rechecked when the
provider changes its on-disk contract.

### Plugin Pass-Through

Wardian does not class-filter, install, enable, disable, or globally suppress
Codex plugins. Each agent sees the plugin state in its own `CODEX_HOME`, and
Wardian launches Codex without global plugin or app disable flags. A plugin or
configuration change takes effect only in a fresh Codex session because the
provider fixes its tool surface when the thread starts. Inspect the effective
state with:

```bash
wardian agent doctor <agent-name-or-uuid>
```

During habitat refresh, Wardian projects the native Codex marketplace catalogs
and plugin cache into the agent home as directory links. The links are
provider-owned implementation assets; agent databases, sessions, configuration
overlays, and plugin enablement state remain local. Native marketplace and MCP
runtime entries are refreshed so existing agents do not retain stale provider
paths, while agent-only MCP entries remain available.

### Session and Status Handling

Wardian materializes fresh Codex history without a bootstrap model turn and
retains the exact provider UUID in the agent's private `CODEX_HOME`. A
Wardian-owned app-server manages the session, and the original Codex terminal
attaches to it. Status tracking observes native turn events, including work
started from that terminal.

The [agent messaging tools](./developer/agent-messaging-tools.md) separate
information, follow-up tasks, and interruption. Delivery uses the shared
local app-server connection. The local-daemon integration targets stable CLI
`0.154.0` or later, with a tested `0.154.0-alpha.6` exception and actual runtime
capability checks. Information does not start a turn, and a task receipt
does not imply a reply. Existing embedded terminal sessions adopt this runtime
on an explicit restart; uncertain messages are not automatically replayed.

Resuming a Codex agent applies its selected model and reasoning effort. If only
the effort is selected, Wardian keeps the conversation's recorded model when
available, then falls back to the effective provider configuration and catalogue.
This launch-time selection does not replace an unspecified model in agent settings.

Codex currently limits the length of its local control-socket path. When a
managed home is too deep, Wardian uses a private compact location and keeps
`habitat/.codex` linked to it. The provider executable and the user's shell
environment remain unchanged. An ownership record in the agent directory
identifies the physical home; include that target when backing up provider
state. Startup reports an error if no secure location fits. This temporary
workaround is tracked for removal in
[#1235](https://github.com/wardian-app/Wardian/issues/1235).

### Debug First

If startup reports `list_turns is not supported yet`, the installed Codex
runtime cannot resume that conversation's paginated history. Closing other
apps does not fix this compatibility error. Keep the original history and use
a compatible runtime or a separately backed-up recovery; do not delete the
conversation to clear its red status. Intentional Wardian shutdown does not
mark healthy shared Codex connections as provider errors.

If a resumed conversation reports `already has an active writer`, another
Codex process still owns that conversation. Release it in the other Codex app
or terminal, then restart the Wardian agent. If the other app retains ownership,
quit it after saving other work. Do not delete the rollout or writer-lock file;
the owning process must release the lock. Restore failures are also recorded in
`<wardian-home>/wardian_debug.log`.

If Codex behaves unexpectedly, run `wardian agent doctor <agent-name-or-uuid>`
first. It reports the agent's effective `CODEX_HOME`, installed/enabled plugins
from that home, launch flags, and a detectable stalled composer. If doctor
reports `provider_input_state: stalled_composer`, run
`wardian agent restart <agent-name-or-uuid>` to clear the pending input while
preserving the agent identity, habitat, and session history. Confirm the failed
turn never started before sending a replacement. If the home changed after the
thread began, start a fresh Codex session before judging the tool list. Then
separate the checks: did it discover the skill, did it trust the real workspace,
and did the sandbox allow the command to run?

## OpenCode (`opencode`)

OpenCode runs directly in the real target workspace and consumes `AGENTS.md` natively.

### Instruction and Skill Discovery

Wardian injects provider runtime configuration through a generated `OPENCODE_CONFIG` file and runtime config directory. The config adds extra Wardian instruction files to `instructions`, and `OPENCODE_CONFIG_DIR` exposes projected common, class, and agent skills without repository-local copies.

### Session and Status Handling

Wardian discovers OpenCode session IDs from JSON events emitted by `opencode run --format json`, then uses `--session <session_id>` for resumes and headless follow-up runs. Interactive terminal telemetry is supported, with provider-specific output cleanup for TUI rendering behavior.

### Debug First

If OpenCode misses instructions or skills, inspect the generated `OPENCODE_CONFIG` file and `OPENCODE_CONFIG_DIR` skill projection. On Windows, also verify whether Wardian resolved a native executable or correctly wrapped a command shim through the host shell.

## Pi (`pi`)

Pi runs directly in the real target workspace and keeps the user's global Pi
configuration, authentication, packages, extensions, and themes.

### Instruction and Skill Discovery

Pi reads project and parent `AGENTS.md` files natively. Wardian passes each
common, class, and agent `AGENTS.md` through repeated
`--append-system-prompt <absolute-file-path>` arguments and passes each
Wardian-managed `.agents/skills` directory through repeated
`--skill <absolute-directory-path>` arguments. The target repository stays
unchanged.

### Session and Status Handling

Wardian gives every fresh Pi launch a provider UUID distinct from the Wardian
agent UUID. The provider session JSONL is isolated under the agent's Wardian
directory through `--session-dir`; fresh launches use `--session-id` and resumed
launches use `--session`. Wardian watches only the JSONL whose header confirms
that exact ID.

Visible agents use Pi's `regular` TUI mode so xterm retains terminal scrollback.
Automation execution uses `--mode json` and treats `agent_end` as definitive turn
completion. Model discovery uses `pi --list-models`; models that advertise
thinking support expose Pi's `off` through `max` levels in the model picker.

Pi's project trust controls project-local settings, extensions, and skills. It
does not sandbox tools or extensions. On Windows, Pi also requires a
Bash-compatible shell, normally Git Bash.

### Debug First

If Pi starts but Chat or `wardian agent watch` has no transcript, inspect the
agent's `pi/sessions` directory and verify that the JSONL header ID matches the
saved `resume_session`. If no model is selectable, run `pi --list-models` in a
normal terminal and complete provider authentication or configure a custom
model. If tool execution fails on Windows, verify Pi's Bash setup before
changing Wardian's PTY path.

## Gemini CLI (`@google/gemini-cli`) — Unmaintained

> **Unmaintained.** Gemini CLI support is no longer actively maintained. Consumer/free Gemini CLI access cut off on June 18, 2026. For Google-model access, use **Antigravity** (`agy`) instead — it is the preferred replacement, uses the same `AGENTS.md`-based instruction model, and receives active support.

Gemini runs directly in the real target workspace.

### Instruction and Skill Discovery

Gemini reads `GEMINI.md`. Wardian passes common, class, and agent include roots through `--include-directories`. The Gemini skill patch lets the CLI discover skills from those additional roots rather than only from the global or project-local Gemini skill folders.

### Session and Status Handling

Wardian learns Gemini session identity from provider output and parses Gemini stream events into lifecycle states such as initialization, user input, generation, and turn completion. Automation execution uses these structured turn-completion signals instead of waiting for fragile terminal text.

### Debug First

If Gemini misses Wardian-managed skills, check the Gemini patch state and include roots before changing workspace or automation logic.

## Related References

- [Developer Provider Runtime Notes](./developer/provider-runtimes.md)
- [Provider Readiness](./guide/provider-readiness.md)
- [Settings](./guide/settings.md)
- [Agent Roles and Responsibilities](./agents/roles.md)
