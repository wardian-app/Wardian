# Provider Runtime Notes

This document captures the practical runtime differences between Wardian's supported CLI providers: Gemini, Antigravity, Claude, Codex, and OpenCode. It is intended for maintainers working on spawn, resume, workflow execution, skill projection, and status/approval handling.

## Shared Wardian Invariants

- The Rust backend remains the source of truth for provider process lifecycle, session IDs, PTY ownership, and status telemetry.
- Every provider receives Wardian's `system_include_directories`, which are resolved from `common`, `classes/<class>`, and `agents/<session_id>`.
- Headless execution and interactive execution use the same provider-specific assumptions where possible. Differences should stay explicit in `manager.rs` instead of being hidden in frontend state.
- Provider-native instruction discovery matters more than Wardian's abstract model. The backend adapts Wardian's files and directories to each CLI instead of expecting the CLI to understand Wardian directly.
- Workflow Agent nodes expose one run mode: `ephemeral`, `inherit_fresh`, or `inherit_resume`. Provider resume flags are emitted only for `inherit_resume`.
- `inherit_fresh` clones the selected agent's runtime configuration and scoped read context, but writes workflow artifacts under a workflow-run session ID and clears provider resume state.
- Workflow-spawned fresh runs skip interactive startup prompts. The workflow node prompt is the first provider input.
- Regular visible agents use the global `Regular agent sessions` setting unless the agent config sets `session_persistence` to `fresh` or `resume`. The agent-level `default` value inherits the global setting.
- The regular-agent context menu `Clear` action forces a fresh provider launch for that one action and clears both the backend PTY output buffer and frontend terminal scrollback cache.

## Quick Comparison

| Provider | Working root | Instruction file | Skill model | Session identity |
| --- | --- | --- | --- | --- |
| Gemini | Real target workspace | `GEMINI.md` | Patched CLI can discover skills from include directories | Discovered from provider output |
| Antigravity | Real target workspace | `AGENTS.md` | `--add-dir` roots expose Wardian context | Discovered from conversation cache and transcript path |
| Claude | Real target workspace | `CLAUDE.md` | `.claude/skills` points at Wardian's `.agents/skills` | Wardian assigns `--session-id` up front |
| Codex | Real target workspace via `--cd` | `AGENTS.md` | Per-agent `CODEX_HOME/skills` under habitat | Discovered from provider output, then adopted |
| OpenCode | Real target workspace | `AGENTS.md` plus injected runtime config | `skills.paths` built from Wardian include roots | Discovered from provider output |

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

## Antigravity

### Working-root model

Antigravity runs directly in the real target workspace. Wardian does not use a projected workspace for Antigravity.

### Instruction and context discovery

- Antigravity reads `AGENTS.md`.
- Wardian passes common, class, and agent include roots as repeated `--add-dir <absolute-path>` flags.
- The provider adapter intentionally stays separate from Gemini even though Antigravity stores runtime files under `~/.gemini/antigravity-cli`.

### Session and telemetry behavior

- Visible launches use `agy --prompt-interactive`.
- Headless launches use `agy --print <prompt>`.
- Resume launches pass `--conversation <conversation-id>`.
- Antigravity may write the useful assistant response only to `brain/<conversation-id>/.system_generated/logs/transcript.jsonl`.
- Wardian discovers the active conversation from `cache/last_conversations.json` or the newest `brain/<conversation-id>` directory, stores that ID as `resume_session`, and parses completed `MODEL` `PLANNER_RESPONSE` transcript records for status and `wardian agent watch` transcript text.

### Practical implications

- Do not use Gemini's `--include-directories`, `--session-id`, or stream output assumptions for Antigravity.
- Empty stdout from `agy --print` can still be a successful run if the transcript contains the answer.
- Auth or account polling warnings in Antigravity logs can be non-fatal; verify transcript output before declaring the run blocked.

## Claude

### Working-root model

Claude also runs directly in the real target workspace. Wardian does not use a projected workspace for Claude.

### Session identity

- Fresh Claude spawns use an explicit Wardian-generated `--session-id`.
- This avoids a bootstrap phase just to discover the provider session ID.
- Resume launches use `--resume <session_id>` and do not resend `--session-id` or `--name`.
- Fresh resume of an existing Wardian agent uses a new transient Claude provider session ID while keeping the Wardian agent ID stable. After launch, Wardian stores the transient Claude ID as the next `resume_session`.

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
- On Windows, Claude may invoke both PowerShell and bash-family tool shells during one Wardian-managed session. Wardian therefore installs both `%USERPROFILE%\.wardian\bin\wardian.cmd` and `%USERPROFILE%\.wardian\bin\wardian`, then prepends the active Wardian `bin` directory to the managed provider process PATH. Verify shell parity from inside the managed runtime, not only from the parent Wardian process.
- Windows manual smoke:

```powershell
powershell -NoProfile -Command "wardian --version"
bash -lc "wardian --version"
```

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

- shared Codex files projected into each agent home:
  - `auth.json`
  - `config.toml`
  - `cap_sid`
- Codex session and history files such as `history.jsonl`,
  `session_index.jsonl`, and `sessions/**` remain per-agent so Wardian can
  resume and read logs from the agent habitat without merging independent
  provider threads.
- Codex SQLite databases such as `state_5.sqlite*` and `logs_2.sqlite*` remain
  per-agent because SQLite journal/WAL files are path-sensitive and are not safe
  to hardlink across independent `CODEX_HOME` directories.
- Codex runtime directories such as `log`, cache, temp, and
  generated database files remain per-agent.
- On Windows, Codex elevated sandbox support is treated separately from session
  state:
  - `.sandbox-secrets` and `.sandbox-bin` are projected from the user's Codex
    home so every Wardian-created Codex home sees the same elevated sandbox
    credentials and helpers.
  - `.sandbox/setup_marker.json` is copied when present so a new projected home
    can observe completed setup.
  - `.sandbox` itself is not projected. Runtime files such as `sandbox.log` and
    `setup_error.json` stay local to the agent or bootstrap home.
- Codex system skills remain under `CODEX_HOME/skills/.system`
- Wardian-assigned skills are projected into `CODEX_HOME/skills/<skill-name>`

This preserves per-agent skill scope without forcing the project repo itself to hold agent-specific skill directories.

### Session identity and bootstrap

Codex session IDs are still discovered from provider output, so fresh session creation has a bootstrap phase.

Current sequence:

1. Create a temporary bootstrap `CODEX_HOME` under `.wardian/provider-bootstrap/codex/session-*/.codex`.
2. Seed it with shared Codex auth/trust files and Windows sandbox support files when present.
3. Launch Codex with the real workspace as `--cd`.
4. Parse `thread.started` to get the provider session ID.
5. Create the final habitat at `.wardian/agents/<provider_session_id>/habitat/.codex`.
6. Project any bootstrap-generated Windows sandbox support into the final home.
7. Migrate session artifacts from the bootstrap home into the final agent home while leaving reusable sandbox support in the bootstrap home.

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

## OpenCode

### Working-root model

OpenCode runs directly in the real target workspace. Wardian does not use a projected workspace for OpenCode.

### Instruction and skill discovery

- OpenCode reads `AGENTS.md` natively when it exists in the working tree.
- Wardian also injects runtime configuration through `OPENCODE_CONFIG_CONTENT`.
- That injected config adds:
  - extra `AGENTS.md` files from Wardian include roots to `instructions`
  - extra `.agents/skills` directories from Wardian include roots to `skills.paths`

This is how OpenCode sees Wardian-managed class and agent context without forcing those files into the user repository.

### Session identity

- OpenCode session IDs are discovered from JSON output during `opencode run --format json`.
- Wardian extracts the first `sessionID` it sees from `step_start` events.
- Resume uses `--session <session_id>`.

### Practical implications

- OpenCode is closer to Gemini than Codex on workspace handling: it wants the real repo as `cwd`.
- OpenCode is closer to Codex than Gemini on instruction naming: it consumes `AGENTS.md` directly.
- If OpenCode stops seeing Wardian skills or class instructions, inspect the generated `OPENCODE_CONFIG_CONTENT` first.
- If interactive spawn works but telemetry is thin, that is expected today; OpenCode does not expose one stable per-session JSONL path the way Claude and Codex do.
- On Windows, Wardian should prefer a native `opencode.exe` or packaged OpenCode binary over `.cmd`/script shims during PATH resolution. If only a shim exists, interactive launch must wrap it through `cmd /d /c ...` because direct PTY spawning does not get shell dispatch semantics.

## Choosing Where to Debug

When provider behavior breaks, start with the provider-specific seam instead of the generic agent UI.

- Gemini problems: inspect patching, include directories, and JSON event parsing.
- Claude problems: inspect `CLAUDE.md` discovery, permission hooks, and explicit session flags.
- Codex problems: inspect `CODEX_HOME`, `--cd`, bootstrap migration, and sandbox approval transitions.
- OpenCode problems: inspect `OPENCODE_CONFIG_CONTENT`, real-workspace `cwd`, and JSON session parsing.
