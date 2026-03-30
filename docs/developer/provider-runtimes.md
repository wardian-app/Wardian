# Provider Runtime Notes

This document captures the practical runtime differences between Wardian's three supported CLI providers: Gemini, Claude, and Codex. It is intended for maintainers working on spawn, resume, workflow execution, skill projection, and status/approval handling.

## Shared Wardian Invariants

- The Rust backend remains the source of truth for provider process lifecycle, session IDs, PTY ownership, and status telemetry.
- Every provider receives Wardian's `system_include_directories`, which are resolved from `common`, `classes/<class>`, and `agents/<session_id>`.
- Headless execution and interactive execution use the same provider-specific assumptions where possible. Differences should stay explicit in `manager.rs` instead of being hidden in frontend state.
- Provider-native instruction discovery matters more than Wardian's abstract model. The backend adapts Wardian's files and directories to each CLI instead of expecting the CLI to understand Wardian directly.

## Quick Comparison

| Provider | Working root | Instruction file | Skill model | Session identity |
| --- | --- | --- | --- | --- |
| Gemini | Real target workspace | `GEMINI.md` | Patched CLI can discover skills from include directories | Discovered from provider output |
| Claude | Real target workspace | `CLAUDE.md` | `.claude/skills` points at Wardian's `.agents/skills` | Wardian assigns `--session-id` up front |
| Codex | Real target workspace via `--cd` | `AGENTS.md` | Per-agent `CODEX_HOME/skills` under habitat | Discovered from provider output, then adopted |

## Gemini

### Working-root model

Gemini runs directly in the real target workspace. Wardian does not project a habitat workspace for Gemini.

### Instruction and skill discovery

- Gemini reads `GEMINI.md`.
- Wardian passes include roots through `--include-directories`.
- Skill discovery depends on Wardian's Gemini patching flow; see [Gemini CLI Patches](./gemini-cli-patches.md).
- If Gemini stops seeing Wardian-managed skills, check the patched CLI bundle before changing spawn logic.

### Session and telemetry behavior

- Gemini session identity is learned from provider output rather than assigned before launch.
- Wardian parses Gemini JSON events into `Init`, `UserQuery`, `Generating`, and `TurnCompleted` states.

### Practical implications

- Gemini is the least habitat-dependent provider.
- Regressions here are usually about CLI patch drift, include-directory handling, or event parsing, not workspace projection.

## Claude

### Working-root model

Claude also runs directly in the real target workspace. Wardian does not use a projected workspace for Claude.

### Session identity

- Fresh Claude spawns use an explicit Wardian-generated `--session-id`.
- This avoids a bootstrap phase just to discover the provider session ID.
- Resume launches use `--resume <session_id>` and do not resend `--session-id` or `--name`.

### Instruction and skill discovery

- Claude reads `CLAUDE.md`.
- Wardian enables `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` so Claude can discover instruction files from `--add-dir` roots.
- Wardian also maintains `.claude/skills -> .agents/skills` links where needed so provider-native skill discovery still works.

### Approval handling

- Claude permission requests are surfaced through a generated hook under `.wardian/agents/<session_id>/claude/`.
- The hook writes permission request events to a JSONL file that Wardian watches.
- If Claude appears stuck in approval state, inspect the hook output before changing status code.

### Practical implications

- Claude depends heavily on the permission-hook path being writable and stable.
- Bugs here are usually about hook setup, `CLAUDE.md` discovery, or resume/session flags.

## Codex

### Working-root model

Codex must run with the real project workspace as its effective working root. Wardian now enforces this by passing `--cd <real workspace>` for interactive spawn, headless resume, and bootstrap session creation.

Wardian still keeps Codex state in a per-agent habitat:

- final agent home: `.wardian/agents/<provider_session_id>/habitat/.codex`
- temporary bootstrap home: `.wardian/provider-bootstrap/codex/session-*/.codex`

The critical rule is: **trust should bind to the real workspace, not to the bootstrap directory or habitat path**.

### Skill discovery model

Codex does not treat `--add-dir` as a skill-discovery mechanism. Wardian therefore projects assigned skills into the agent-specific `CODEX_HOME/skills` tree.

Current model:

- shared Codex files copied into each agent home:
  - `auth.json`
  - `config.toml`
  - `cap_sid`
- Codex system skills remain under `CODEX_HOME/skills/.system`
- Wardian-assigned skills are projected into `CODEX_HOME/skills/<skill-name>`

This preserves per-agent skill scope without forcing the project repo itself to hold agent-specific skill directories.

### Session identity and bootstrap

Codex session IDs are still discovered from provider output, so fresh session creation has a bootstrap phase.

Current sequence:

1. Create a temporary bootstrap `CODEX_HOME` under `.wardian/provider-bootstrap/codex/session-*/.codex`.
2. Seed it with shared Codex auth/trust files.
3. Launch Codex with the real workspace as `--cd`.
4. Parse `thread.started` to get the provider session ID.
5. Create the final habitat at `.wardian/agents/<provider_session_id>/habitat/.codex`.
6. Migrate session artifacts from the bootstrap home into the final agent home.

If Codex starts asking for trust every launch again, first verify that the session was born with the real workspace as `cwd`, not the bootstrap path.

### Approval and status handling

Codex emits several different event shapes across live PTY output and persisted session logs.

Wardian treats these as the important lifecycle markers:

- `thread.started`: session identity available
- `turn.started`: processing begins
- `exec_approval_request` or escalated `function_call`: action required
- `exec_command_begin`, `exec_command_start`, `function_call_output`: processing resumes after approval
- `task_complete` / `turn.completed`: idle

Codex commentary events like `agent_message` should not be used as hard status transitions.

### Known operational edge cases

- Codex skill discovery can be correct while shell execution is still blocked by the CLI sandbox. In that case, the agent sees the skill but fails when the skill tries to invoke shell tools.
- On Windows, those failures may surface as `CreateProcessAsUserW failed: 5` or setup-helper launch errors.
- When debugging Codex, separate these questions explicitly:
  - Did Codex discover the skill?
  - Did Codex trust the workspace?
  - Did Codex succeed in spawning a shell command under its sandbox?

## Choosing Where to Debug

When provider behavior breaks, start with the provider-specific seam instead of the generic agent UI.

- Gemini problems: inspect patching, include directories, and JSON event parsing.
- Claude problems: inspect `CLAUDE.md` discovery, permission hooks, and explicit session flags.
- Codex problems: inspect `CODEX_HOME`, `--cd`, bootstrap migration, and sandbox approval transitions.
