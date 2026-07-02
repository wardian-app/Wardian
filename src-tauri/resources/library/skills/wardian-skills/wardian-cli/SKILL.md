---
name: wardian-cli
description: "Use immediately when a request mentions Wardian, Wardian agents, other agents, peers, delegation, orchestration, workflows, agent identity, agent status, agent workspaces, live or persisted Wardian state, the Wardian CLI, or any interaction from inside a Wardian-managed terminal."
---

# Wardian CLI

Use the `wardian` command as the first source of truth for Wardian state and
peer-agent coordination. Prefer it over guessing from UI state, terminal titles,
or filesystem inspection.

## First Moves

When a task involves Wardian or another agent:

1. Inspect yourself if you are inside a Wardian-managed terminal.
2. Inspect the live roster with `wardian agent list` (shows your neighbors by default).
   Use `--scope all` only when your task genuinely spans multiple neighbor sets (e.g., you are an
   orchestrator wiring up new agents).
3. Pick an idle, suitable peer by class, provider, workspace, and status.
4. Spawn an explicit class/provider peer when no suitable peer exists.
5. Send bounded instructions, wait or poll for completion, collect the response,
   then kill temporary agents when they are no longer needed.

```bash
wardian agent
wardian agent list --scope all --fields name,uuid,class,provider,workspace,status,status_source
wardian agent <name-or-uuid> --fields name,uuid,class,provider,workspace,status,status_source
```

`wardian agent` and `wardian agent show` without a target mean "show the current
agent." They require `WARDIAN_SESSION_ID`, so they usually work only inside a
Wardian-managed agent terminal.

From an ordinary terminal, pass a target:

```bash
wardian agent show Wardian-Codex
wardian agent show 019d331a-0500-7592-969f-8f437886f42b
```

## Agent Listing And Output

```bash
wardian agent list
wardian agent list --scope all
wardian agent list --scope all --status idle
wardian agent list --scope all --class Coder
wardian agent list --workspace <absolute-workspace-path>
wardian agent worktree list
```

By default, `wardian agent list` shows your **neighbors** — the agents you're
connected to through the communication topology (manual edges, your teams, or your
workspace if you're not yet wired into the graph). This shapes your default attention
without restricting capability. Bare-name agent sends resolve within your neighbors
first, then fall back to global exact match.

**Scope modes:**
- `--scope auto` (default): neighbors when inside a Wardian-managed session, else workspace.
- `--scope neighbors`: self + direct topology neighbors (manual edges, team cliques, workspace fallback).
- `--scope workspace`: all agents in your workspace.
- `--scope all`: all known agents (use only for orchestration tasks that span multiple neighbor sets).

Use `--scope all` only when your task genuinely spans multiple neighbor sets or you're
coordinating across workspaces.

Default output is indented JSON with `schema: 1`. Use `--field` for
shell-friendly bare values:

```bash
wardian agent Wardian-Codex --field status
wardian agent Wardian-Codex --field workspace
```

Use `--fields` to request only specific JSON fields:

```bash
wardian agent list --scope all --fields name,status
wardian agent list --scope all --fields name,status,status_source
```

`status_source` is hidden by default. Request it when you need to know whether
the answer came from the running desktop app or persisted state:

- `live` means the running desktop app answered.
- `persisted` means the CLI fell back to `state.db`.

Use `--verbose` for `pid`, `started_at`, and `last_status_at`. Use `--pretty`
only for human-readable terminal inspection; automation should keep JSON.

## Lifecycle Control

Mutating commands use Wardian's local control endpoint and require the desktop
app to be running for the same `WARDIAN_HOME`.

```bash
wardian agent spawn --provider codex --class Reviewer --name reviewer-a1 --workspace <absolute-workspace-path>
wardian agent clone coder-a1 --name coder-a2
wardian agent worktree enable coder-a1 --name review-fixes
wardian agent worktree join coder-a1 --worktree <absolute-worktree-path-or-id>
wardian agent worktree disable coder-a1
wardian agent pause reviewer-a1
wardian agent resume reviewer-a1
wardian agent kill reviewer-a1
wardian agent wait reviewer-a1 --until idle --timeout 10m
wardian agent wait reviewer-a1 --until idle --next --timeout 10m
wardian agent watch reviewer-a1 --until output:REVIEW_DONE --include status,output --timeout 10m
wardian ask reviewer-a1 --stdin --timeout 10m
wardian reply ask_0123456789abcdef --status done --stdin
```

`agent spawn` requires both `--provider` and `--class`; do not rely on implicit
defaults when creating agents. `agent clone` copies the source agent's provider,
class, workspace, and context unless the CLI offers an override for the field
you need.

`agent wait <target> --until <status>` blocks until a single agent name or UUID
reaches a normalized status such as `idle`, `processing`, `action_required`,
`off`, or `error`. Plain `wait` returns immediately when the agent is already
in the requested status. Use `--next` to wait for a newer matching observation.
Use `--timeout` with `ms`, `s`, or `m` units.

`agent watch <target>` returns status, provider-adapted transcript text,
sanitized terminal output, delivery details, and a cursor by default. Raw PTY
text is opt-in with `--raw` or `--include raw_output`; use it only when you need
terminal escape bytes or repaint evidence. Use `--until output:<token>` when you
need output-substring compatibility; marker matching checks transcript text,
sanitized output, and the internal raw PTY fallback. `--follow` is reserved and
currently returns `not_supported`.

`agent worktree` commands require the desktop app for the same `WARDIAN_HOME`.
They route through Wardian's live control endpoint and reuse the GUI/backend
worktree logic. `enable`, `join`, and `disable` clear the target agent session
after moving its workspace so the provider starts fresh in the new location.
`disable` removes only the assignment; it does not delete the physical worktree
folder.

Use `wardian ask` for one-off peer tasks where you need delivery evidence and a
structured reply from the target:

```bash
cat <<'EOF' | wardian ask reviewer-a1 --stdin --timeout 10m
Review this patch.
EOF
```

`ask` accepts one agent name or UUID, captures a pre-send watch cursor, sends
the prompt with a backend-owned `request_id`, and waits for the target to run
`wardian reply <request-id> --status done --stdin`. The response JSON includes
`request_id`, `reply.status`, `reply.body`, `delivery`, watch `events`, and
retained `output`. `reply.status` can be `done`, `blocked`, or `failed`;
timeouts remain separate `watch_timeout` errors.

Use `--until output:<token>` only when you explicitly need output-substring
matching for manual compatibility. Other explicit watch conditions such as
`status:<status>`, `event:<kind>`, and `delivery:<state>` also preserve the
watch-based behavior. Broadcasts, class selectors, and `--thread` are not
supported by `ask` in this slice.

When responding to a structured ask from inside a Wardian-managed agent
terminal, use:

```bash
cat <<'EOF' | wardian reply ask_0123456789abcdef --status done --stdin
Reviewed the patch. No blocking findings.
EOF
```

Wardian verifies the sender identity when `WARDIAN_SESSION_ID` is available.
Replies from ordinary terminals are accepted in this first live-only slice so a
human can unblock a request, but that caller identity is not authenticated.

## Sending Messages

Use `wardian send` for live inter-agent communication:

```bash
wardian send "review this patch" --to reviewer-a1
wardian send --stdin --to reviewer-a1
wardian send --file prompt.md --to reviewer-a1
wardian send --as-command "/goal test" --to reviewer-a1
wardian send "status?" --to class:Coder
wardian send "stand down" --to all
wardian send "review this patch" --to reviewer-a1 --wait-until idle --timeout 10m
```

Targets can be an agent name, UUID, `class:<ClassName>`, or `all`. By default:
- `--to all` broadcasts to your **neighbors** (not global).
- `--to class:Coder` resolves within your neighbors.
- Bare names resolve neighbors-first; if no neighbor matches, fall back to global.
- Explicit UUIDs and exact names always work regardless of topology (soft boundary).

Use `--scope all` on `send` only when you need global broadcast/class resolution for
orchestration across multiple neighbor sets. Use `--wait-until` only with a single-agent target;
broadcasts are for messages that should not block the current command.

Normal sends preserve inter-agent attribution when Wardian knows the sender.
Use `--as-command` when delivering provider slash commands that must be the
first input token:

```bash
wardian send --as-command "/goal test" --to reviewer-a1
printf '%s' '/status' | wardian send --stdin --as-command --to reviewer-a1
```

PowerShell:

```powershell
"/status" | wardian send --stdin --as-command --to reviewer-a1
```

`--as-command` sends the exact message body without a `From <sender>:` prefix,
while still using the provider-aware submit path. It accepts only one explicit
agent name or UUID and rejects `all`, `class:<ClassName>`, and `--thread` with
`not_supported`.

Successful sends include `delivery[]`. Failed delivery emits JSON on stderr with
`details.delivery[]`, including `runtime_state`, `delivery_state`,
`input_mode`, and any input channel error. Successful command sends also expose
`input_mode: "command"` in the response for automation.

`--thread` is reserved for grouped conversations. Until threading is implemented
end-to-end, the running app rejects it with `not_supported` instead of silently
dropping it.

For substantial prompts, prefer `--stdin` or `--file` so quoting does not damage
the instruction:

```bash
cat <<'EOF' | wardian send --stdin --to reviewer-a1 --wait-until idle --timeout 10m
Review the changes since origin/main.
Return findings first, then tests run, then any residual risk.
EOF
```

On PowerShell, use a here-string instead of a POSIX heredoc:

```powershell
@"
Review the changes since origin/main.
Return findings first, then tests run, then any residual risk.
"@ | wardian send --stdin --to reviewer-a1 --wait-until idle --timeout 10m
```

## Workflows

```bash
wardian workflow list
wardian workflow show <id-or-name>
wardian workflow run <id>
wardian workflow stop <run-instance-id>
```

`workflow list` and `workflow show` try the running app first, then read
workflow JSON files from disk when the app is unavailable. `workflow run` and
`workflow stop` require the running app.

Malformed workflow files may be reported as warnings or omitted depending on
the CLI version. If `show` cannot find a workflow that exists on disk, inspect
the workflow JSON for parse errors.

## Teams And Watchlists

```bash
wardian team list
wardian team show <team-name-or-id>
wardian watchlist list
wardian watchlist show <watchlist-name-or-id>
```

These commands are read-only and inspect the persisted `watchlists/index.json`
file. They accept the current v2 watchlist state with global teams and legacy
flat watchlist arrays. Do not assume `team:<name>` send targeting exists yet;
use explicit agent names/UUIDs or `class:<ClassName>` until team send targeting
is implemented.

## Orchestration Pattern

For multi-agent work, route through Wardian instead of handling every subtask in
one terminal. Orchestrators typically use `--scope all` to discover and coordinate
across communities:

```bash
wardian agent list --scope all --fields name,class,provider,workspace,status
wardian agent spawn --provider codex --class Reviewer --name review-cli-surface --workspace <absolute-workspace-path>
cat <<'EOF' | wardian ask review-cli-surface --stdin --until output:REVIEW_DONE --timeout 10m
Review this patch. End with REVIEW_DONE.
EOF
wardian agent kill review-cli-surface
```

Good delegated tasks are bounded and independently checkable: code review,
targeted repository exploration, documentation audits, verification runs, and
implementation slices with disjoint file ownership. Keep urgent blocking work
local when your next step depends on it, but use peers aggressively for parallel
analysis and validation.

Always tell temporary peers what output shape you need and where to send it.
After waiting, verify status and read the peer's response from the active
terminal, transcript, or returned control output when available. Real PTY input
delivery can vary by provider and platform, especially on Windows, so treat
timeouts or missing responses as delivery failures and report them plainly.

## Exit Codes And Errors

Errors are JSON on stderr. Common cases:

- `not_in_session`: self lookup was requested outside a Wardian-managed process.
  Pass an explicit name or UUID.
- `not_found`: the requested name or UUID was not found. Run
  `wardian agent list --scope all --fields name,uuid`.
- `ambiguous`: a name matched multiple agents. Use the UUID.
- `db_unavailable`: no live app answered and `state.db` was unavailable.
- `app_not_running`: a live-control command could not reach the desktop app.
  This maps to exit code 6.
- `not_supported`: the command shape is recognized but not implemented by the
  running app yet, such as `send --thread`.

## Development Notes

When testing a dev app and CLI together, set the same `WARDIAN_HOME` in both
terminals. The live control endpoint is keyed by `WARDIAN_HOME`.

macOS/Linux shell:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
npm run dev
```

Second terminal:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all --fields name,status,status_source
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
npm run dev
```

Second terminal:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all --fields name,status,status_source
```

After a release build from this workspace, use repo-root `target` outputs:

```bash
./target/release/wardian-cli agent list --scope all
```

On Windows release builds, the binaries use `.exe` names:

```powershell
.\target\release\wardian-cli.exe agent list --scope all
```
