# Provider Runtime Notes

This document captures the practical runtime differences between Wardian's supported CLI providers: Antigravity, Claude, Codex, OpenCode, Pi, and Gemini (unmaintained). It is intended for maintainers working on spawn, resume, automation execution, skill projection, and status/approval handling.

## Shared Wardian Invariants

- The Rust backend remains the source of truth for provider process lifecycle, session IDs, PTY ownership, and status telemetry.
- Every provider receives Wardian's `system_include_directories`, which are resolved from `common`, `classes/<class>`, and `agents/<session_id>`.
- Headless execution and interactive execution use the same provider-specific assumptions where possible. Differences should stay explicit in `manager.rs` instead of being hidden in frontend state.
- Provider-native instruction discovery matters more than Wardian's abstract model. The backend adapts Wardian's files and directories to each CLI instead of expecting the CLI to understand Wardian directly.
- Automation Agent nodes expose one run mode: `ephemeral`, `inherit_fresh`, or `inherit_resume`. Provider resume flags are emitted only for `inherit_resume`.
- `inherit_fresh` clones the selected agent's runtime configuration and scoped read context, but writes automation artifacts under an automation-run session ID and clears provider resume state.
- Automation-spawned fresh runs skip interactive startup prompts. The automation node prompt is the first provider input.
- Regular visible agents use the global `Regular agent sessions` setting unless the agent config sets `session_persistence` to `fresh` or `resume`. The agent-level `default` value inherits the global setting.
- The regular-agent context menu **New Session** action forces a fresh provider launch for that one action and clears both the backend PTY output buffer and frontend terminal scrollback cache. It retains the Wardian agent, habitat, and saved history.
- Provider delivery profiles are responsible for translating Wardian input into the provider's native submit behavior, including short prompts, pasted multiline prompts, long prompts, slash-command-shaped text, and inputs that already end with a newline.
- Delivery recognizers must fail closed. If Wardian cannot recognize that a provider prompt is ready, that a paste bracket has settled, or that a command was submitted, it should avoid sending more input instead of guessing and corrupting the provider TUI state.
- Approval prompt state must be fresh. A stale recognizer hit, old transcript event, or previous terminal buffer line must not keep an agent in `action_required` or trigger a delivery retry for a new turn.
- On Windows, provider adapters should prefer direct native executables or a
  direct `node <script.js>` launch resolved from an npm `.cmd` shim. Shell-wrap
  only when shell dispatch is required, such as extensionless OpenCode shims.

## Quick Comparison

| Provider | Working root | Instruction file | Skill model | Session identity |
| --- | --- | --- | --- | --- |
| Antigravity | Real target workspace | `AGENTS.md` | `--add-dir` roots expose Wardian context | Captured after the first real prompt |
| Claude | Real target workspace | `CLAUDE.md` | `.claude/skills` points at Wardian's `.agents/skills` | Wardian assigns `--session-id` up front |
| Codex | Real target workspace via `--cd`; habitat-backed `CODEX_HOME` | `AGENTS.md` | Per-agent `CODEX_HOME/skills` under habitat | Fresh local rollout, then exact resume |
| OpenCode | Habitat command root; real workspace passed as a positional arg (interactive) or `--dir` (headless `run`) | `AGENTS.md` plus injected runtime config | Skills junctioned into the habitat `.opencode` config dir | Discovered from provider output (`ses_…`) |
| Pi | Real target workspace | `AGENTS.md` plus appended Wardian instruction files | Repeated `--skill` paths point at Wardian-managed skill roots | Wardian assigns `--session-id` up front |
| Gemini *(unmaintained)* | Projected habitat workspace for headless runs | `GEMINI.md` | Patched CLI can discover skills from include directories | Discovered from provider output |

## Antigravity

### Model discovery

Wardian reads `agy models` from the installed CLI. Current releases return
tab-separated model IDs and display names; both are preserved in the model
picker. Progress messages are excluded. The selected ID is passed unchanged
through `--model` for launches, including low-effort Flash variants. OpenCode's
one-model-ID-per-line catalog remains supported by the shared parser.

### Working-root model

Antigravity runs directly in the real target workspace. Wardian does not use a projected workspace for Antigravity.

### Instruction and context discovery

- Antigravity reads `AGENTS.md`.
- Wardian passes common, class, and agent include roots as repeated `--add-dir <absolute-path>` flags.
- The provider adapter intentionally stays separate from Gemini even though Antigravity stores runtime files under `~/.gemini/antigravity-cli`.
- Hidden Wardian roots are projected through visible temp paths before they are passed to `agy`. If a projected root contains `.agents/skills`, Wardian materializes that root and follows deployed skill links so Antigravity sees real skill directories instead of junctions or symlinks back into hidden storage.
- `deploy_skill` and `remove_deployed_skill` refresh live Antigravity projections after the canonical Wardian skill tree changes. The library skill watcher also refreshes projections after skill-file changes while it is active. Agent restart remains the full rebuild path for projections.

### Session and telemetry behavior

- Visible launches use `agy --prompt-interactive`.
- Headless launches use `agy --print <prompt>`.
- Antigravity presents an interactive first-use folder-trust modal even with its tool-permission bypass flag. Wardian confirms only that exact startup modal once when managed permission bypass is enabled. This keeps Antigravity's normal workspace-to-conversation mapping intact for strict provider identity capture.
- Wardian-managed launches separately pass `--dangerously-skip-permissions` by default to auto-approve tool permission requests. Set `dangerously_skip_permissions` to `false` explicitly to retain both folder-trust and per-tool operator approval prompts.
- Resume launches pass `--conversation <conversation-id>`.
- Current Antigravity releases persist interactive user and assistant steps in `conversations/<conversation-id>.db`; Wardian projects newly observed provider-authored rows into live watch state. The older `brain/<conversation-id>/.system_generated/logs/transcript.jsonl` remains a compatibility fallback.
- New Session starts Antigravity fresh without sending a bootstrap prompt. Wardian first accepts Antigravity's changed workspace cache mapping when available. Antigravity 1.1.22 can leave that cache stale, so Wardian can instead bind the sole post-launch conversation DB whose provider-authored trajectory metadata contains the exact workspace URI. Ambiguous or pre-launch databases are rejected. The verified ID is then stored as `resume_session`.
- Until a real prompt creates that mapping, no provider identity exists to resume; a restart starts a fresh conversation again.
- Wardian verifies Antigravity's exact workspace-cache mapping against `conversation_metadata.json`, then resumes that conversation with `--conversation`. A conversation explicitly detached by **Clear** is excluded from recovery.
- Antigravity 1.1.7 and later persist interactive turns in `conversations/<conversation-id>.db`; Wardian binds a known conversation's database for live status as soon as its `steps` schema exists, while Chat waits for a real user-message step before preferring it over the older `brain/<conversation-id>/.system_generated/logs/transcript.jsonl` fallback. Fresh identity discovery still requires exact post-launch workspace metadata and an unambiguous database candidate. Restored agents position the watch cursor after existing rows instead of replaying history as live output.
- The Chat view also replays Wardian's durable conversation archive before the bounded live provider data, so already captured rows remain visible when a provider artifact is temporarily unavailable.
- The real-provider rendering audit uses a short exact marker prompt for Antigravity, submits it through Wardian's provider-aware prompt delivery path, and treats the post-clear respawn as marker-optional. This avoids mistaking echoed prompt text for the model response while still proving initial live rendering, resize, pause, and resume behavior.

### Prompt delivery

- Antigravity's editor honors bracketed paste. Wardian wraps multiline prompts, and single-line prompts of 2048 bytes or more, in `ESC[200~` … `ESC[201~`, then sends one carriage return as a separate write. Short single-line prompts keep the simple literal path.
- This supersedes an earlier assumption that Antigravity did not support bracketed paste. That assumption made Wardian send long multiline prompts literally, so the editor treated the embedded newlines as submits and could retain the prompt unsent with no turn produced. A native protocol experiment against Antigravity 1.1.27 sent raw `ESC[200~ payload ESC[201~` for a 6886-byte, 285-line prompt; the editor collapsed it into a single paste entry, and one carriage return produced a provider-native answer containing all three independent random labels placed at the payload's beginning, middle, and end.
- The 500 ms submit settle delay is unchanged. The experiment's 267 ms editor-application time is one machine's measurement, not a guarantee, so delivery still depends on the existing bounded turn receipt and still fails closed with no automatic retry when that receipt does not arrive.

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
- Ordinary habitat preparation materializes existing owned common/class/agent
  `CLAUDE.md` bridges from sibling canonical `AGENTS.md`; habitat generation and
  the subsequent memory append also refresh the habitat bridge. These are
  bootstrap snapshots, with no live refresh guarantee. Exact legacy stubs and
  unchanged versioned/hash-marked projections are eligible; customized files and
  links are preserved. Nested imports are copied verbatim and retain provider
  consent. See the [operator freshness rules](../providers.md#instruction-and-skill-discovery-1).
- Wardian also maintains `.claude/skills -> .agents/skills` links where needed so provider-native skill discovery still works.
- Wardian enables `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1` for Claude launches in Wardian-managed terminal surfaces so mobile and remote terminal scrollback remains native to xterm.

### Approval handling

- Wardian-managed launches use Claude's `bypassPermissions` mode by default. Claude can present a separate first-run safety-consent selector for that mode; Wardian rejects the selector as readiness evidence and confirms only that exact startup modal once. Delivery remains queued until Claude's real compose surface is available and any visible `/rc connecting…` transition has completed. Set an explicit mode such as `manual`, `acceptEdits`, or `plan` to opt back into provider approval behavior.
- Claude permission requests are surfaced through a generated hook under `.wardian/agents/<session_id>/claude/`.
- The hook writes permission request events to a JSONL file that Wardian watches.
- If Claude appears stuck in approval state, inspect the hook output before changing status code.

### Practical implications

- Claude depends heavily on the permission-hook path being writable and stable.
- Bugs here are usually about hook setup, `CLAUDE.md` discovery, or resume/session flags.
- If mobile or remote drag scrolling fails only for Claude, verify that the managed launch environment still includes the alternate-screen opt-out before changing terminal gesture handling.
- On Windows, Claude may invoke both PowerShell and bash-family tool shells during one Wardian-managed session. Wardian therefore installs both `%USERPROFILE%\.wardian\bin\wardian.cmd` and `%USERPROFILE%\.wardian\bin\wardian`, then prepends the active Wardian `bin` directory to the managed provider process PATH. Verify shell parity from inside the managed runtime, not only from the parent Wardian process.
- If `%USERPROFILE%\bin\wardian` or `%USERPROFILE%\bin\wardian.cmd` is a Wardian-owned legacy launcher, the Windows installer rewrites it to forward to the active `%USERPROFILE%\.wardian\bin\wardian-cli.exe`. This protects Claude bash tool shells that prepend `~/bin` ahead of the inherited provider PATH.
- Windows manual smoke:

```powershell
powershell -NoProfile -Command "wardian --version"
bash -lc "wardian --version"
```

### Headless result output

Claude's verbose JSON output can contain an event array ending in a result.
Wardian extracts that terminal answer and its provider session ID before
passing the answer to automation. Intermediate messages remain diagnostic
data. Existing single-object responses are also supported. Missing, ambiguous
or unsuccessful terminal results fail the task instead of exposing event data
as a successful answer.

## Codex

### Chat history

- Startup host-context records do not start work or increment the query count. The status parser and Chat share the native content-kind classifier; explicit user content, legacy string prompts, and canonical user events retain their normal activity behavior. See [#1249](https://github.com/wardian-app/Wardian/issues/1249).
- Codex emits a lightweight `agent_message` and a completed `response_item` for the same visible assistant response. The completed record can append an internal `<oai-mem-citation>` block. Wardian removes that block before storing or rendering the message, and applies the same normalization while replaying older archived rows, so one user-visible answer appears once.
- Wardian memory rows are filtered to the active conversation boundary before they are merged into Chat, then receive the same chronological sequence assignment as provider and watch events. Agent-wide memory history must not be replayed into a later conversation.

### Working-root model

Codex must run with the real project workspace as its effective working root. Wardian now enforces this by passing `--cd <real workspace>` for interactive spawn, headless resume, and bootstrap session creation.

Wardian still keeps Codex state in a per-agent habitat:

- final agent home: `.wardian/agents/<wardian-agent-id>/habitat/.codex`
- legacy fallback bootstrap home: `.wardian/provider-bootstrap/codex/session-*/.codex`

The critical rule is: **trust should bind to the real workspace, not to the bootstrap directory or habitat path**.

### Skill discovery model

Codex does not treat `--add-dir` as a skill-discovery mechanism. Wardian therefore projects assigned skills into the agent-specific `CODEX_HOME/skills` tree.

Current model:

- shared Codex files copied into each agent home:
  - `auth.json`
  - `cap_sid`
- the user's `config.toml` is a managed base, not a shared home. Wardian
  reconciles missing base policy values into the agent's own `config.toml` and
  preserves agent model choices, project trust, and local overrides.
- Codex projects `sessions/**` from the native Codex home into each agent home
  through a directory link (a Windows junction on Windows). Existing local
  rollouts are copied first without changing their filenames. If link creation
  fails, the local sessions tree is restored and the provider continues in
  local-only mode.
- The provider writes agent-local `history.jsonl` and `session_index.jsonl`.
  Wardian is the sole writer to the central copies: it publishes complete,
  validated records under a cross-process lock, atomically republishes the
  complete central file, repairs invalid central tails, and de-duplicates repeat
  observations.
- Codex SQLite databases such as `state_5.sqlite*` and `logs_2.sqlite*` remain
  per-agent because SQLite journal/WAL files are path-sensitive and are never
  shared or hardlinked. Runtime logs, databases, and temporary files remain
  local, except for the provider-owned marketplace catalogs and plugin cache
  directories explicitly projected below.
- `auth.json` and `cap_sid` flow only from the native Codex home into an agent
  home; they are never copied back outward.
- Codex runtime directories such as `log`, temp, and generated database files
  remain per-agent. The provider plugin cache is the explicit exception
  described below.
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
- Native Codex marketplace catalogs under `.tmp/bundled-marketplaces` and
  `.tmp/plugins`, plus `plugins/cache`, are projected into each agent home as
  directory links. These are provider-owned implementation assets, not shared
  agent state.

This preserves per-agent skill scope without forcing the project repo itself to hold agent-specific skill directories.

### Plugin pass-through and diagnostics

Plugin enablement and agent-specific configuration remain entirely in each
agent's `CODEX_HOME`. Wardian does not apply a class allowlist, does not alter
installed or enabled plugin state, and does not pass global plugin/app disable
flags.
The native marketplace catalogs and plugin implementation cache are projected
as provider-owned directory links so every agent can resolve the same current
plugin surface without copying agent databases. Agent-local configuration is
reconciled with current marketplace and MCP runtime records; agent-only MCP
entries remain intact. A configuration or plugin change needs a new Codex
session because an existing thread has a fixed tool list.

Use the provider-neutral control surface to inspect effective state without
reading sensitive Codex files:

```bash
wardian agent doctor <agent-name-or-uuid>
```

The response includes the effective home path, installed/enabled plugins read
from that home through the provider-resolved Codex executable, and launch
feature flags. It never changes plugin state.

### Session identity and bootstrap

Codex fresh-session materialization does not require a model bootstrap turn.

Current sequence:

1. Create or update the agent's projected `CODEX_HOME` under `.wardian/agents/<wardian-agent-id>/habitat/.codex`.
2. Generate a distinct provider UUID and write a minimal `session_meta` rollout at `sessions/<year>/<month>/<day>/rollout-<timestamp>-<provider-id>.jsonl`.
3. Validate that Codex resolves the rollout from that same projected home.
4. Start the Wardian-owned local daemon without loading a thread. The ordinary
   TUI resumes the selected provider UUID under the same home and real workspace.
   Wardian requires that exact thread to appear in the owned daemon before
   subscribing and enabling peer delivery. No model bootstrap turn is required.

Legacy bootstrap migration remains available as a fallback when local rollout materialization is unavailable. It merges a new rollout into an existing projected `sessions/**` tree instead of discarding it.

If Codex starts asking for trust every launch again, first verify that the session was born with the real workspace as `cwd`, not the bootstrap path.

Wardian also exposes an off-by-default global **Trust launch workspaces** Codex
runtime setting. When enabled, Wardian passes a launch-scoped config override
for the agent workspace:

```bash
codex -c 'projects."<absolute-agent-workspace-path>".trust_level="trusted"'
```

PowerShell:

```powershell
codex -c 'projects."<absolute-agent-workspace-path>".trust_level="trusted"'
```

This uses Codex's project trust table without editing the user's global Codex
config file. Keep it separate from Codex autonomous mode: autonomous mode
bypasses approvals and sandboxing, while workspace trust only marks the launch
folder as trusted.

### Approval and status handling

Wardian's Codex approval setting is shared by global runtime defaults and
explicit per-agent overrides. `on-request`, `untrusted`, and `never` are passed
through `--ask-for-approval`; **Approve for me** is translated to Codex's
`--approve-for-me` flag, which selects the workspace-write sandbox and automatic
review. It is never passed as an argument value to `--ask-for-approval`.

Codex emits several different event shapes across live PTY output and persisted session logs.

Wardian treats these as the important lifecycle markers:

- `thread.started`: session identity available
- `turn.started`: processing begins
- `exec_approval_request` or escalated `function_call`: action required
- `exec_command_begin`, `exec_command_start`, `function_call_output`: processing resumes after approval
- `task_complete` / `turn.completed`: idle

Codex commentary events like `agent_message` should not be used as hard status transitions.

The shared app-server event reader observes turns started through either the
TUI or Wardian. A named Wardian inbox output without an originating model
call is non-waking context; it must not mark an idle agent as processing.
V2 peer information, follow-up work, and interruption use native WebSocket
operations through a private local socket and the configured Codex executable's
`app-server proxy` tunnel. MCP exposes the model-facing
tools and explicit receiver. See [agent messaging tools](./agent-messaging-tools.md).

For long canonical homes, owner startup recovers pending launch settings and
then prepares a private compact physical home before config/MCP projection.
The logical habitat path remains an owned directory link. Matching agent and
slot records authorize that link; arbitrary links remain invalid. A separate
preparation lock fences migration against ordinary refresh and index writes.
Refresh resolves completed mappings without moving homes or recovering a live
startup overlay. The provider executable, normal TUI invocation and shell `HOME`
are unchanged. See [the removal criteria](https://github.com/wardian-app/Wardian/issues/1235).

### Known operational edge cases

- Codex skill discovery can be correct while shell execution is still blocked by the CLI sandbox. In that case, the agent sees the skill but fails when the skill tries to invoke shell tools.
- On Windows, those failures may surface as `CreateProcessAsUserW failed: 5` or setup-helper launch errors.
- When debugging Codex, separate these questions explicitly:
  - Did Codex discover the skill?
  - Did Codex trust the workspace?
  - Did Codex succeed in spawning a shell command under its sandbox?

## OpenCode

### Working-root model

OpenCode uses a Wardian habitat as the provider command root when projected context is available. The real target workspace is passed explicitly: as a positional directory argument for interactive TUI launches, and as `--dir` for headless `opencode run` invocations.

### Instruction and skill discovery

- OpenCode reads `AGENTS.md` natively when it exists in the working tree.
- Wardian writes a runtime config file to `<habitat>/.opencode/opencode.json` and points OpenCode at it through `OPENCODE_CONFIG` (plus `OPENCODE_CONFIG_DIR`).
- That injected config adds extra `AGENTS.md` files from Wardian include roots to `instructions`.
- Skills from Wardian include roots are junctioned into the config dir's `skills/` folder. There is no `skills.paths` config key: OpenCode 1.4.3 dropped it, so Wardian omits any `skills` key entirely.

This is how OpenCode sees Wardian-managed class and agent context without forcing those files into the user repository.

### Session identity

- OpenCode session IDs are discovered from JSON output during `opencode run --format json`, or captured from `opencode session list` while the interactive TUI runs.
- Valid IDs match `ses_…`; Wardian never substitutes its own UUIDs into `--session`.
- Resume uses `--session <session_id>`.

### Headless prompt input

Wardian sends the complete headless OpenCode prompt as UTF-8 on stdin, then
closes the pipe to signal EOF. It supplies no positional message: OpenCode's
`run` parser reconstructs positional messages with literal quotes. Whitespace,
line endings, quotes, backslashes, and Unicode therefore remain part of the
original prompt. Model, agent, session, output-format, and directory flags still
use the ordinary argument path.

The execution deadline and conversation-lease heartbeat also cover blocked
stdin writes. Failed or cancelled delivery terminates the owned process tree;
a failed write can represent partial delivery and is never automatically retried
by this transport. OpenCode failure errors retain the exit code but omit raw
provider stderr, which can echo private input. Input fidelity does not guarantee
that the provider's answer satisfies the requested task.

### Practical implications

- OpenCode is closer to Gemini than Codex on workspace handling: Wardian launches from the habitat command root while passing the real repo as the project directory.
- OpenCode is closer to Codex than Gemini on instruction naming: it consumes `AGENTS.md` directly.
- If OpenCode stops seeing Wardian skills or class instructions, inspect the generated `<habitat>/.opencode/opencode.json` (`OPENCODE_CONFIG`) first, then verify the junctioned `skills/` entries resolve.
- Interactive status comes from TUI window-title scraping ("OpenCode" idle, "OC | …" processing), while token/cost telemetry comes from OpenCode's shared SQLite store via wardian-core; both channels are expected to exist side by side.
- TUI "Permission required" prompts never appear in the window title. Wardian detects them from the provider log (`message=asking id=per_…`) and raises Action Needed; the ask is attributed to a session only while its prompt loop is the sole open loop in the log, and clears once loop activity resumes after the prompt is answered.
- On Windows, Wardian should launch the `opencode` command resolved from PATH,
  matching how a user terminal starts OpenCode. Interactive and headless launch
  wrap that command through the configured shell because npm and PowerShell
  shims need shell dispatch semantics.

## Pi

### Working-root and identity model

Pi runs in the real workspace. Wardian passes a private `--session-dir` under
the Wardian agent directory, assigns a distinct UUID with `--session-id`, and
uses `--session` for exact resume. Pi writes the JSONL lazily after its first
persisted entry, so discovery validates the session header ID and never falls
back to the newest file.

### Instruction and skill model

Pi discovers the workspace's parent/project `AGENTS.md` chain itself. Wardian
adds common, class, and agent instructions with repeated
`--append-system-prompt <absolute-file-path>` arguments and their Agent Skills
directories with repeated `--skill <absolute-directory-path>` arguments. Do not
redirect `PI_CODING_AGENT_DIR`: it would replace the user's authentication,
packages, extensions, settings, and themes rather than isolate only sessions.

### Output and status model

The interactive `regular` TUI stays attached to the PTY. Wardian tails the
version 3 session JSONL for user messages, assistant stop reasons, tool calls,
tool results, and definitive completion. Headless automations use `--mode json`;
`agent_end`, not a transient assistant message, is the final automation boundary.

Pi's `--approve` and `--no-approve` flags control project-local configuration,
extensions, and skills. They do not sandbox the shell tool or extensions.

## Gemini (Unmaintained)

> **Unmaintained.** Consumer/free Gemini CLI access ended June 18, 2026. Use Antigravity for Google-model access — it is the preferred replacement.

### Working-root model

Gemini headless runs use a projected habitat workspace so shared, class, and
agent instructions and skills can be materialized outside Wardian's hidden state
tree. The real target workspace remains the author-facing workspace, but the
provider process runs from the habitat workspace path during headless execution.

### Instruction and skill discovery

- Gemini reads `GEMINI.md`.
- Wardian passes include roots through `--include-directories`.
- Skill discovery depends on Wardian's Gemini patching flow; see [Gemini CLI Patches](./gemini-cli-patches.md).
- If Gemini stops seeing Wardian-managed skills, check the patched CLI bundle before changing spawn logic.

### Session and telemetry behavior

- Gemini session identity is learned from provider output rather than assigned before launch.
- Wardian parses Gemini JSON events into `Init`, `UserQuery`, `Generating`, and `TurnCompleted` states.

### Practical implications

- Gemini regressions are usually about habitat projection, CLI patch drift,
  include-directory handling, or event parsing.

## Choosing Where to Debug

When provider behavior breaks, start with the provider-specific seam instead of the generic agent UI.

- Antigravity problems: inspect visible include-root projections, the exact workspace conversation mapping, and the conversation transcript.
- Claude problems: inspect `CLAUDE.md` discovery, permission hooks, and explicit session flags.
- Codex problems: inspect `CODEX_HOME`, `--cd`, bootstrap migration, and sandbox approval transitions.
- OpenCode problems: inspect the generated `OPENCODE_CONFIG` file, junctioned skills, real-workspace directory argument, and `ses_…` session discovery.
- Pi problems: inspect appended system-prompt and skill arguments, the private session directory, and the exact Pi session ID and JSONL boundary.
- Gemini problems (unmaintained): inspect patching, include directories, and JSON event parsing.
