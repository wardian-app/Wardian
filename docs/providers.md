# Provider Runtimes

Wardian provides one orchestration layer over five supported CLI providers: Antigravity, Claude Code, Codex, OpenCode, and Gemini CLI. Each provider keeps its native command-line behavior, while Wardian adapts session identity, working roots, skill discovery, status tracking, and workflow execution into a consistent app model.

## Overview

| Provider | Support | Working Root | Instruction Source | Skill and Context Model | Session Identity |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **[Antigravity](https://www.antigravity.google/docs/cli-overview)** | Supported | Real target workspace | `AGENTS.md` | Wardian include roots passed through repeated `--add-dir` flags | Discovered from Antigravity conversation state |
| **[Claude Code](https://github.com/anthropics/claude-code)** | Supported | Real target workspace | `CLAUDE.md` | `--add-dir` instruction roots plus `.claude/skills` links to Wardian-managed skills | Wardian assigns fresh session IDs and resumes explicitly |
| **[Codex](https://github.com/openai/codex)** | Supported | Real target workspace via `--cd` | `AGENTS.md` | Per-agent `CODEX_HOME` habitat with scoped skill projection | Discovered during bootstrap, then adopted into the final habitat |
| **[OpenCode](https://github.com/anomalyco/opencode)** | Supported | Real target workspace | `AGENTS.md` plus injected runtime config | `OPENCODE_CONFIG` adds Wardian instructions; `OPENCODE_CONFIG_DIR` exposes projected skills | Discovered from JSON events and resumed with `--session` |
| **[Gemini CLI](https://github.com/google-gemini/gemini-cli)** | Unmaintained | Real target workspace | `GEMINI.md` | Wardian include roots passed through `--include-directories`; Gemini patch enables multi-root skill discovery | Discovered from provider output |

## Shared Runtime Model

- The Rust backend is the source of truth for process lifecycle, provider session IDs, PTY ownership, and status telemetry.
- Regular visible agents use the global session policy unless the agent has an explicit override.
- Workflow Agent nodes choose one run mode: `ephemeral`, `inherit_fresh`, or `inherit_resume`.
- Wardian keeps user repositories clean by adapting provider-native discovery instead of copying agent-specific instruction and skill files into the project root.

## Antigravity (`agy`)

Antigravity runs directly in the real target workspace.

### Instruction and Skill Discovery

Antigravity reads `AGENTS.md`. Wardian passes common, class, and agent include roots with repeated `--add-dir <absolute-path>` flags so the CLI can load Wardian-managed context without copying agent files into the repository.

Wardian-managed roots usually live under hidden `.wardian` directories. Antigravity can ignore or under-discover hidden include roots, so Wardian exposes those roots through visible temp projections under the system temp directory before passing them to `agy`. Roots that contain `.agents/skills` are materialized into that projection instead of linked directly, because Antigravity does not reliably discover skills that are nested links back into hidden Wardian storage. Skill deploy/remove operations refresh live Antigravity projections, and the library skill watcher refreshes projections after skill-file changes while it is active. Restarting the agent rebuilds the projection from the canonical Wardian roots.

### Session and Status Handling

Wardian launches visible Antigravity agents with `agy --prompt-interactive ""` so the CLI starts in interactive mode without an initial task. Headless workflow runs use `agy --print` and, when resuming, `--conversation <conversation-id>`. Provider options include `--sandbox`, `--dangerously-skip-permissions`, and `--print-timeout <duration>`.

Antigravity stores runtime state under `~/.gemini/antigravity-cli`. Wardian discovers the conversation ID from the provider cache and reads `brain/<conversation-id>/.system_generated/logs/transcript.jsonl` for status, assistant transcript text, and tool activity. `wardian agent watch` uses completed `MODEL` `PLANNER_RESPONSE` transcript records as provider-adapted assistant output, planner `tool_calls` as tool-call rows, and model action records such as `RUN_COMMAND`, `VIEW_FILE`, `CODE_ACTION`, `SEARCH_WEB`, `LIST_DIRECTORY`, `GREP_SEARCH`, `READ_URL_CONTENT`, `ASK_QUESTION`, and `GENERIC` as tool-result rows.

### Debug First

If Antigravity starts but Wardian does not show assistant text, inspect the conversation cache and transcript path above. If `agy --print` returns empty stdout, check the transcript before treating the run as failed.

## Claude Code (`@anthropic-ai/claude-code`)

Claude runs directly in the real target workspace.

### Instruction and Skill Discovery

Claude reads `CLAUDE.md`. Wardian enables additional-directory discovery and maintains `.claude/skills` links where needed so Claude can see Wardian-managed common, class, and agent skills without those files living in the repository root.

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

Each agent keeps its own Codex home. Wardian reconciles shared configuration defaults into
that home without copying another agent's sessions, history, databases, workspace
trust, or local overrides.

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

### Session and Status Handling

Codex session IDs are discovered after launch. Wardian starts fresh sessions with a temporary bootstrap `CODEX_HOME`, parses the provider session ID, creates the final per-agent habitat, and migrates session artifacts there. Status tracking uses Codex thread and turn events, approval requests, command events, and completion markers.

### Debug First

If Codex behaves unexpectedly, run `wardian agent doctor <agent-name-or-uuid>`
first. It reports the agent's effective `CODEX_HOME`, installed/enabled plugins
from that home, and launch flags. If the home changed after the thread began,
start a fresh Codex session before judging the tool list. Then separate the
checks: did it discover the skill, did it trust the real workspace, and did the
sandbox allow the command to run?

## OpenCode (`opencode`)

OpenCode runs directly in the real target workspace and consumes `AGENTS.md` natively.

### Instruction and Skill Discovery

Wardian injects provider runtime configuration through a generated `OPENCODE_CONFIG` file and runtime config directory. The config adds extra Wardian instruction files to `instructions`, and `OPENCODE_CONFIG_DIR` exposes projected common, class, and agent skills without repository-local copies.

### Session and Status Handling

Wardian discovers OpenCode session IDs from JSON events emitted by `opencode run --format json`, then uses `--session <session_id>` for resumes and headless follow-up runs. Interactive terminal telemetry is supported, with provider-specific output cleanup for TUI rendering behavior.

### Debug First

If OpenCode misses instructions or skills, inspect the generated `OPENCODE_CONFIG` file and `OPENCODE_CONFIG_DIR` skill projection. On Windows, also verify whether Wardian resolved a native executable or correctly wrapped a command shim through the host shell.

## Gemini CLI (`@google/gemini-cli`) — Unmaintained

> **Unmaintained.** Gemini CLI support is no longer actively maintained. Consumer/free Gemini CLI access cut off on June 18, 2026. For Google-model access, use **Antigravity** (`agy`) instead — it is the preferred replacement, uses the same `AGENTS.md`-based instruction model, and receives active support.

Gemini runs directly in the real target workspace.

### Instruction and Skill Discovery

Gemini reads `GEMINI.md`. Wardian passes common, class, and agent include roots through `--include-directories`. The Gemini skill patch lets the CLI discover skills from those additional roots rather than only from the global or project-local Gemini skill folders.

### Session and Status Handling

Wardian learns Gemini session identity from provider output and parses Gemini stream events into lifecycle states such as initialization, user input, generation, and turn completion. Workflow execution uses these structured turn-completion signals instead of waiting for fragile terminal text.

### Debug First

If Gemini misses Wardian-managed skills, check the Gemini patch state and include roots before changing workspace or workflow logic.

## Related References

- [Developer Provider Runtime Notes](./developer/provider-runtimes.md)
- [Provider Readiness](./guide/provider-readiness.md)
- [Settings](./guide/settings.md)
- [Agent Roles and Responsibilities](./agents/roles.md)
