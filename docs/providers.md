# Provider Runtimes

Wardian provides one orchestration layer over five supported CLI providers: Gemini CLI, Antigravity, Claude Code, Codex, and OpenCode. Each provider keeps its native command-line behavior, while Wardian adapts session identity, working roots, skill discovery, status tracking, and workflow execution into a consistent app model.

## Overview

| Provider | Support | Working Root | Instruction Source | Skill and Context Model | Session Identity |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **[Gemini CLI](https://github.com/google-gemini/gemini-cli)** | Supported | Real target workspace | `GEMINI.md` | Wardian include roots passed through `--include-directories`; Gemini patch enables multi-root skill discovery | Discovered from provider output |
| **[Antigravity](https://www.antigravity.google/docs/cli-overview)** | Supported | Real target workspace | `AGENTS.md` | Wardian include roots passed through repeated `--add-dir` flags | Discovered from Antigravity conversation state |
| **[Claude Code](https://github.com/anthropics/claude-code)** | Supported | Real target workspace | `CLAUDE.md` | `--add-dir` instruction roots plus `.claude/skills` links to Wardian-managed skills | Wardian assigns fresh session IDs and resumes explicitly |
| **[Codex](https://github.com/openai/codex)** | Supported | Real target workspace via `--cd` | `AGENTS.md` | Per-agent `CODEX_HOME` habitat with scoped skill projection | Discovered during bootstrap, then adopted into the final habitat |
| **[OpenCode](https://github.com/anomalyco/opencode)** | Supported | Real target workspace | `AGENTS.md` plus injected runtime config | `OPENCODE_CONFIG` adds Wardian instructions; `OPENCODE_CONFIG_DIR` exposes projected skills | Discovered from JSON events and resumed with `--session` |

## Shared Runtime Model

- The Rust backend is the source of truth for process lifecycle, provider session IDs, PTY ownership, and status telemetry.
- Regular visible agents use the global session policy unless the agent has an explicit override.
- Workflow Agent nodes choose one run mode: `ephemeral`, `inherit_fresh`, or `inherit_resume`.
- Wardian keeps user repositories clean by adapting provider-native discovery instead of copying agent-specific instruction and skill files into the project root.

## Gemini CLI (`@google/gemini-cli`)

Gemini runs directly in the real target workspace.

### Instruction and Skill Discovery

Gemini reads `GEMINI.md`. Wardian passes common, class, and agent include roots through `--include-directories`. The Gemini skill patch lets the CLI discover skills from those additional roots rather than only from the global or project-local Gemini skill folders.

### Session and Status Handling

Wardian learns Gemini session identity from provider output and parses Gemini stream events into lifecycle states such as initialization, user input, generation, and turn completion. Workflow execution uses these structured turn-completion signals instead of waiting for fragile terminal text.

### Debug First

If Gemini misses Wardian-managed skills, check the Gemini patch state and include roots before changing workspace or workflow logic.

### Migration Note

Keep Gemini CLI support while users transition provider choices. Consumer/free Gemini CLI access is scheduled to cut off on June 18, 2026. Antigravity support is a separate provider path and does not reuse the Gemini adapter.

## Antigravity (`agy`)

Antigravity runs directly in the real target workspace.

### Instruction and Skill Discovery

Antigravity reads `AGENTS.md`. Wardian passes common, class, and agent include roots with repeated `--add-dir <absolute-path>` flags so the CLI can load Wardian-managed context without copying agent files into the repository.

Wardian-managed roots usually live under hidden `.wardian` directories. Antigravity can ignore or under-discover hidden include roots, so Wardian exposes those roots through visible temp projections under the system temp directory before passing them to `agy`. Roots that contain `.agents/skills` are materialized into that projection instead of linked directly, because Antigravity does not reliably discover skills that are nested links back into hidden Wardian storage. Skill deploy/remove operations refresh live Antigravity projections, and the library skill watcher refreshes projections after skill-file changes while it is active. Restarting the agent rebuilds the projection from the canonical Wardian roots.

### Session and Status Handling

Wardian launches visible Antigravity agents with `agy --prompt-interactive ""` so the CLI starts in interactive mode without an initial task. Headless workflow runs use `agy --print` and, when resuming, `--conversation <conversation-id>`. Provider options include `--sandbox`, `--dangerously-skip-permissions`, and `--print-timeout <duration>`.

Antigravity stores runtime state under `~/.gemini/antigravity-cli`. Wardian discovers the conversation ID from the provider cache and reads `brain/<conversation-id>/.system_generated/logs/transcript.jsonl` for status and assistant transcript text. `wardian agent watch` uses completed `MODEL` `PLANNER_RESPONSE` transcript records as provider-adapted assistant output.

### Debug First

If Antigravity starts but Wardian does not show assistant text, inspect the conversation cache and transcript path above. If `agy --print` returns empty stdout, check the transcript before treating the run as failed.

## Claude Code (`@anthropic-ai/claude-code`)

Claude runs directly in the real target workspace.

### Instruction and Skill Discovery

Claude reads `CLAUDE.md`. Wardian enables additional-directory discovery and maintains `.claude/skills` links where needed so Claude can see Wardian-managed common, class, and agent skills without those files living in the repository root.

### Session and Status Handling

Wardian assigns fresh Claude session IDs up front and uses explicit resume flags for resumed provider sessions. Claude permission requests are captured through a generated hook under the Wardian agent directory, which lets the UI surface `Action Needed` with request details.

### Debug First

If Claude appears blocked, inspect the permission hook output, `CLAUDE.md` discovery, and resume flags before treating the issue as a generic PTY failure.

## Codex (`@openai/codex`)

Codex executes against the real target workspace while Wardian keeps mutable provider state in an agent habitat.

### Instruction and Skill Discovery

Codex reads `AGENTS.md`. Wardian passes the real project root with `--cd <absolute-workspace-path>` and projects assigned skills into the agent-specific `CODEX_HOME/skills` tree. This keeps skill scope per agent while preserving Codex trust and command execution against the actual repository path.

### Session and Status Handling

Codex session IDs are discovered after launch. Wardian starts fresh sessions with a temporary bootstrap `CODEX_HOME`, parses the provider session ID, creates the final per-agent habitat, and migrates session artifacts there. Status tracking uses Codex thread and turn events, approval requests, command events, and completion markers.

### Debug First

If Codex behaves unexpectedly, separate the checks: did it discover the skill, did it trust the real workspace, and did the sandbox allow the command to run?

## OpenCode (`opencode`)

OpenCode runs directly in the real target workspace and consumes `AGENTS.md` natively.

### Instruction and Skill Discovery

Wardian injects provider runtime configuration through a generated `OPENCODE_CONFIG` file and runtime config directory. The config adds extra Wardian instruction files to `instructions`, and `OPENCODE_CONFIG_DIR` exposes projected common, class, and agent skills without repository-local copies.

### Session and Status Handling

Wardian discovers OpenCode session IDs from JSON events emitted by `opencode run --format json`, then uses `--session <session_id>` for resumes and headless follow-up runs. Interactive terminal telemetry is supported, with provider-specific output cleanup for TUI rendering behavior.

### Debug First

If OpenCode misses instructions or skills, inspect the generated `OPENCODE_CONFIG` file and `OPENCODE_CONFIG_DIR` skill projection. On Windows, also verify whether Wardian resolved a native executable or correctly wrapped a command shim through the host shell.

## Related References

- [Developer Provider Runtime Notes](./developer/provider-runtimes.md)
- [Provider Readiness](./guide/provider-readiness.md)
- [Settings](./guide/settings.md)
- [Agent Roles and Responsibilities](./agents/roles.md)
