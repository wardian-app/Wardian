# Wardian CLI

Wardian includes a standalone `wardian` command for agents and automation to inspect, coordinate, and control known agent sessions. Wardian remains GUI/app-first for humans; the CLI is the textual control surface agents use when they need to discover themselves, coordinate peers, or ask the running app to perform live actions for the same `WARDIAN_HOME`.

Use it when an agent, script, or terminal workflow needs repeatable access to Wardian state without clicking through the desktop UI.

The CLI is also Wardian's composability layer. It lets agents, scripts, and
future tools operate against the same app-owned state as the desktop UI without
screen scraping or duplicating private state. Prefer CLI/backend commands when
building repeatable automation around Wardian.

## When to Use It

- Let a managed agent identify itself with `wardian agent`.
- Send prompts or structured asks from one agent to another.
- Wait for an agent to reach a status or emit a marker.
- Start, stop, or inspect workflows from automation.
- Read persisted teams, watchlists, and agent state when the app is not running.

## Basic Workflow

1. Launch Wardian once so the CLI is installed into the Wardian bin directory.
2. Restart your terminal if `wardian` is not on `PATH`.
3. Set the same `WARDIAN_HOME` in both the app and terminal when using an isolated test home.
4. Run `wardian agent list` to confirm the CLI sees your neighbors, or `wardian agent list --scope all` to see all agents.
5. Use live-control commands only while the desktop app is running for that same home.

## Installation

The desktop app copies the bundled CLI on startup:

- macOS/Linux command: `$HOME/.wardian/bin/wardian`
- macOS/Linux implementation binary: `$HOME/.wardian/bin/wardian-cli`
- Windows command: `%USERPROFILE%\.wardian\bin\wardian.cmd`
- Windows bash command: `%USERPROFILE%\.wardian\bin\wardian`
- Windows implementation binary: `%USERPROFILE%\.wardian\bin\wardian-cli.exe`

Wardian also attempts to add that `bin` directory to the user PATH. On Windows, Wardian installs both a `.cmd` launcher for PowerShell/cmd and an extensionless launcher for bash-family shells such as Git Bash, MSYS2, or provider shell tools that execute `bash`. Wardian-managed agent processes receive the active Wardian `bin` directory at the front of `PATH`, so shell tools inside managed sessions can resolve `wardian` without depending on the user's global shell startup files. Restart ordinary terminals after first launch if `wardian` is not found.

Set `WARDIAN_HOME` to redirect state, the CLI install location, and the live app control endpoint for tests or isolated runs.

For development, `npm run dev` uses the app debug home by default and ignores an inherited default production home from a managed agent shell. Set the same non-production `WARDIAN_HOME` before starting the dev desktop app and before running CLI commands when you want the CLI to inspect that dev app's live state.

When the desktop app is running for the same `WARDIAN_HOME`, the CLI asks the app for live agent snapshots before falling back to `state.db`. Request `status_source` when you need to know which path answered:

- `live` means the status came from the running desktop app.
- `persisted` means the CLI fell back to durable `state.db` state.

## Agent Identity

Wardian injects `WARDIAN_SESSION_ID` into managed agent processes. Inside an agent terminal, `wardian agent` resolves that session automatically.

Outside a managed agent process, pass a name or UUID:

```bash
wardian agent coder-a1
wardian agent show uuid-1
```

## Communication Topology & Scope

Wardian maintains a communication topology that shapes which agents you see and interact with by default. Your **neighbors** are determined by the graph topology: manual edges (including team-seeded edges) or your workspace-mates (if you have no manual edges).

**Why it matters:**
- `wardian agent list` shows your neighbors by default — the agents you're connected to — so you work within your context.
- `wardian send --to all` broadcasts within your neighbors, not globally.
- `wardian send --to class:Coder` resolves within your neighbors.
- Bare-name targets resolve neighbors-first; explicit UUIDs and exact names always work regardless of topology.

**Scope modes for `wardian agent list`:**
- `--scope auto` (default): neighbors when `WARDIAN_SESSION_ID` is set (inside a Wardian agent terminal), else workspace.
- `--scope neighbors`: self + direct topology neighbors (manual edges, workspace fallback when you have no manual edges).
- `--scope workspace`: all agents in your workspace.
- `--scope all`: all known agents across all workspaces.

**When to use each scope:**
- Default (`auto`): Normal agent work within your context (neighbors inside a session, workspace outside).
- `--scope neighbors`: Explicit neighbors-only listing (same as auto inside a session).
- `--scope workspace`: When you need to see all agents in your workspace regardless of edges.
- `--scope all`: Only for orchestration tasks that genuinely span multiple neighbor sets or workspaces.

When you create a team or add a team member, Wardian automatically wires up edges between all team members in the topology. These connections shape your default visibility and are completely editable through the Graph view. See the [Graph](./graph.md) view for the visual control surface: create and delete connections, view your neighbors, and inspect the topology source at `<WARDIAN_HOME>/topology.json`.

## Commands

```bash
wardian agent
wardian agent <name-or-uuid>
wardian agent show [name-or-uuid]
wardian agent list
wardian agent list --scope all
wardian agent kill <name-or-uuid>
wardian agent pause <name-or-uuid>
wardian agent resume <name-or-uuid>
wardian agent spawn --provider codex --class Reviewer --name reviewer-a1 --workspace <absolute-workspace-path>
wardian agent clone <name-or-uuid> --name coder-a2
wardian agent worktree list
wardian agent worktree enable <name-or-uuid> --name review-fixes
wardian agent worktree join <name-or-uuid> --worktree <absolute-worktree-path-or-id>
wardian agent worktree disable <name-or-uuid>
wardian agent wait reviewer-a1 --until idle --timeout 10m
wardian agent wait reviewer-a1 --until idle --next --timeout 10m
wardian agent watch reviewer-a1 --until output:REVIEW_DONE --include transcript,output,delivery --timeout 10m
wardian agent watch reviewer-a1 --include raw_output --raw
wardian team list
wardian team show <team-name-or-id>
wardian watchlist list
wardian watchlist show <watchlist-name-or-id>
wardian workflow node-types
wardian workflow validate <path-to-workflow.md>
wardian workflow exec <path-to-library-workflow.md> --provider codex --workspace <absolute-workspace-path>
wardian workflow runs
wardian workflow run-show <blueprint-id> <run-id>
wardian workflow replay <blueprint-id> <run-id>
wardian workflow schedule list
wardian conversation list
wardian conversation list --agent <agent-id-or-name>
wardian conversation list --scope all
wardian conversation show <conversation-id>
wardian ask reviewer-a1 --stdin --timeout 10m
wardian reply ask_0123456789abcdef --status done --stdin
wardian send "review this" --to coder-a1
wardian send --as-command "/goal test" --to coder-a1
wardian send "review this" --to reviewer-a1 --wait-until idle --timeout 10m
wardian send "status?" --to class:Coder
wardian send "stand down" --to all
```

## Common Workflows

Inspect your neighbors (default):

```bash
wardian agent list --fields name,class,provider,workspace,status
```

Inspect the full roster when coordinating across multiple neighbor sets:

```bash
wardian agent list --scope all --fields name,class,provider,workspace,status,status_source
```

Hand a bounded review task to a peer and wait for response evidence:

```bash
wardian ask reviewer-a1 --file review-prompt.md --timeout 10m
```

Answer a structured ask from inside the target agent session:

```bash
cat <<'EOF' | wardian reply ask_0123456789abcdef --status done --stdin
Reviewed the patch. No blocking findings.
EOF
```

Send a prompt to an existing agent and wait for the next Idle transition:

```bash
wardian send --file prompt.md --to coder-a1 --wait-until idle --timeout 10m
```

Watch retained readable output for a deterministic marker:

```bash
wardian agent watch coder-a1 --until output:READY_FOR_REVIEW --include transcript,output,delivery --timeout 10m
```

Inspect provider-adapted transcript text, sanitized terminal fallback, or raw PTY evidence:

```bash
wardian agent watch Librarian --include transcript
wardian agent watch Librarian --include output
wardian agent watch Librarian --include raw_output --raw
```

PowerShell:

```powershell
wardian agent watch Librarian --include transcript
wardian agent watch Librarian --include output
wardian agent watch Librarian --include raw_output --raw
```

Run a saved workflow through the app-owned backend:

```bash
wardian workflow validate <absolute-workspace-path>/library/workflows/autoreview.md
wardian workflow exec <absolute-workspace-path>/library/workflows/autoreview.md \
  --provider codex \
  --workspace <absolute-workspace-path> \
  --input '{"target":"HEAD"}' \
  --bind reviewer=codex
wardian workflow runs
wardian workflow run-show autoreview <run-id>
```

PowerShell:

```powershell
wardian workflow validate <absolute-workspace-path>\library\workflows\autoreview.md
wardian workflow exec <absolute-workspace-path>\library\workflows\autoreview.md `
  --provider codex `
  --workspace <absolute-workspace-path> `
  --input '{"target":"HEAD"}' `
  --bind reviewer=codex
wardian workflow runs
wardian workflow run-show autoreview <run-id>
```

By default, `workflow exec` is a live-control command: it requires the desktop app to be running for the same `WARDIAN_HOME`, routes execution through app-owned runtime state, and accepts workflow files under `<wardian-home>/library/workflows`. The `mock` executor is reserved for workflow-engine fixture tests and should not be used as a normal CLI launch path.

Use `workflow runs`, `workflow run-show <blueprint-id> <run-id>`, and `workflow replay <blueprint-id> <run-id>` to inspect durable run artifacts under `<wardian-home>/logs/workflows`.

Use `conversation list` and `conversation show <conversation-id>` to inspect durable agent-owned conversation archives. Inside a Wardian-managed agent terminal, `conversation list` defaults to that agent through `WARDIAN_SESSION_ID`. Outside a managed agent terminal, pass `--agent <agent-id-or-name>` or `--scope all`. `show` returns the manifest and agent-readable `conversation.jsonl` narrative, not provider-private raw logs. Wardian refreshes `turns.jsonl` whenever it refreshes the normalized archive, including open conversations, so readers can use `manifest.json` plus `turns.jsonl` as the cheap per-request index and fall back to `conversation.jsonl` only for full detail. A `turns.jsonl` row means one user-originated request plus following assistant, tool, and lifecycle records until the next user-originated request or boundary; provider tool-call IDs do not create separate turn rows. Context rows such as AGENTS.md injections, goal continuations, and lifecycle-only records are typed in `request.kind` so agents can skip them when building summaries. Agents and external tools should use this CLI surface or bounded reads of `agents/<agent-id>/conversations/index.jsonl`; do not recursively crawl under `agents/*`, because agent directories can contain worktrees, provider caches, screenshots, and dependencies. Direct readers must treat `index.jsonl` as append-only upsert history and keep the latest row per `conversation_id`.

Mutating commands use Wardian's local control endpoint and require the desktop app to be running for the same `WARDIAN_HOME`. This includes agent lifecycle commands, agent worktree commands, live `workflow exec`, and `send`.

`workflow validate`, `workflow parse`, `workflow normalize`, `workflow node-types`, `workflow runs`, `workflow run-show`, `workflow replay`, `conversation list`, and `conversation show` can run from disk without the desktop app.

`agent spawn` requires both `--provider` and `--class` so the created agent's runtime and role are explicit.

`agent worktree list` returns the worktrees currently managed by Wardian with source folder, worktree folder, display name, and member agent IDs. `agent worktree enable`, `join`, and `disable` are live-control commands. They reuse the same backend logic as the Source Control panel and force a fresh agent session after changing the runtime workspace. `disable` removes the assignment only; it does not delete the physical worktree folder.

`team list/show` and `watchlist list/show` read the existing watchlist state file. They accept the current v2 shape with global teams and legacy flat watchlist arrays, then return `schema: 1` JSON for automation. Team mutation and `send --to team:<name>` are not implemented yet.

`agent wait <target> --until <status>` blocks inside the CLI process until a single agent name or UUID reaches a normalized status such as `idle`, `processing`, `action_required`, `off`, or `error`. Plain `wait` returns immediately when the target is already in the requested status. Add `--next` to wait for a newer matching observation. Use `--timeout` with `ms`, `s`, or `m` units.

`agent watch <target>` returns a live snapshot with agent status, a provider-adapted `transcript`, sanitized retained terminal `output`, delivery details, and a cursor. Raw PTY text is not returned by default. Add `--raw` or `--include raw_output` only when debugging terminal rendering, ANSI/control sequences, or PTY transport behavior. `raw_output.text` may contain escape sequences and prompt repaint fragments.

`transcript` is extracted from structured provider lines when Wardian has a provider adapter. This slice covers Codex, Claude, Gemini, Antigravity, OpenCode, and the mock provider. Gemini can backfill completed assistant text from Gemini chat logs, Antigravity can backfill completed assistant text from its conversation transcript, and OpenCode can backfill assistant text from its session database when the live TUI does not expose a clean structured line. Ambiguous provider lines fall back to sanitized terminal `output` until provider-specific transcript adapters are added. `output` is the compatibility surface for `--until output:<substring>` and is cleaned of common ANSI, OSC, cursor, and clear-line controls. Internally, marker matching also checks transcript text and the raw PTY tap so existing token-based automation keeps working without returning raw text by default.

Add `--until` to block until `status:<status>`, `output:<substring>`, `event:<kind>`, or `delivery:<state>` is observed. `watch` accepts only one name or UUID in this slice. `--follow` is reserved and returns `not_supported`.

`ask <target>` sends one prompt to one live Wardian-managed agent and creates a durable task interaction with a backend-owned `request_id`. Wardian appends reply instructions to the delivered prompt and waits for the target to execute `wardian reply <request-id> --status done --stdin`. The structured ask path completes only when the task interaction receives an explicit reply interaction. Echoed request IDs, terminal repaint text, and output markers do not complete the ask.

The JSON response includes `request_id`, `reply.status`, `reply.body`, delivery evidence, watch events, and retained output. `reply.status` can be `done`, `blocked`, or `failed`; timeout remains a separate `watch_timeout` error. If the target runtime is booting, busy, action-required, or missing a safe input channel, Wardian keeps the interaction queued and reports the delivery state instead of relying on a fixed sleep before terminal injection.

Use `--until output:<token>` only when you explicitly need the older output-substring mode, such as manual provider output matching or compatibility with agents that cannot run `wardian reply`. Output markers are weaker evidence than structured replies because they are derived from transcript or terminal output. Other explicit watch conditions such as `status:<status>`, `event:<kind>`, and `delivery:<state>` also preserve the watch-based behavior. `ask` rejects `all`, `class:<ClassName>`, and reserved `--thread` usage with `not_supported`.

`reply <request-id> --status done|blocked|failed --stdin` records a structured reply through the live control endpoint. Wardian resolves the request ID against the interaction store. Unknown request IDs fail deterministically, and duplicate replies are rejected unless a future explicit idempotency policy says otherwise. When run from a Wardian-managed agent terminal, `WARDIAN_SESSION_ID` is used to verify that the reply came from the target agent for that request. Replies submitted outside a Wardian-managed session are accepted for this first live-control slice so a human terminal can unblock a request, but that caller identity is not authenticated.

`send` submits a provider-aware message into the target agent runtime. Targets can be an agent name, UUID, `class:<ClassName>`, or `all`. By default:
- `--to all` broadcasts within your **neighbors**, not globally.
- `--to class:ClassName` resolves within your neighbors.
- Bare agent names resolve neighbors-first; explicit UUIDs always work globally.

Use `--scope all` to broadcast/resolve globally (orchestration across multiple neighbor sets only). `--stdin` reads the message from standard input, and `--file <path>` reads it from a file. By default, Wardian keeps inter-agent attribution and delivers messages with a `From <sender>:` prefix when sender context is available. Use `--as-command` for provider slash commands that must start at the first input token:

```bash
wardian send --as-command "/goal test" --to coder-a1
printf '%s' '/status' | wardian send --stdin --as-command --to coder-a1
```

PowerShell:

```powershell
"/status" | wardian send --stdin --as-command --to coder-a1
```

`--as-command` sends the exact message body without the attribution prefix while still using the normal provider-aware submit path. It accepts only one explicit agent name or UUID, rejects `all` and `class:<ClassName>` with `not_supported`, and cannot be combined with `--thread`.

`--wait-until <status>` is available for single-agent targets and waits from a pre-send watch cursor for a newer matching status observation. `--thread` is reserved but not implemented yet; when the app is running, using it returns `not_supported`.

Successful `send` responses include `input_mode` and `delivery[]`; command sends also include `delivery[].input_mode` so automation can confirm command delivery. Failed or partial delivery returns a nonzero exit with JSON on stderr and `details.delivery[]`, including `runtime_state`, `delivery_state`, and provider-specific input errors.

List filters:

- `--status <status>` filters by normalized status, such as `idle`, `processing`, or `action_required`.
- `--class <class>` filters by agent class.
- `--workspace <absolute-path>` filters by workspace and implies `--scope all`.

Output options:

- `--fields name,status,uuid` returns indented JSON with only those fields.
- `--field status` returns one bare value plus a newline.
- `--field status_source` returns `live` or `persisted`.
- `--verbose` adds `pid`, `started_at`, `last_status_at`, and `visibility` (why each neighbor is visible: `manual` or `rule:workspace-fallback`).
- `--pretty` returns aligned text for interactive inspection instead of JSON.

Default JSON is indented for terminal readability. It includes `schema: 1` and an `agent` or `agents` payload with `name`, `uuid`, `class`, `provider`, `workspace`, and `status`.

## Important Limits

- The desktop app must be running for live-control commands such as `send`, `spawn`, `pause`, `resume`, `kill`, and default `workflow exec`.
- `WARDIAN_HOME` must match between the app and CLI when you expect shared live state.
- Team mutation and `send --to team:<name>` are not implemented yet.
- Raw terminal output can include escape sequences; prefer transcript or sanitized output unless debugging PTY behavior.

## Exit Codes

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | Generic command error |
| 2 | Agent not found |
| 3 | `WARDIAN_SESSION_ID` is not set for self lookup |
| 4 | Wardian state database is unavailable |
| 5 | Lookup matched multiple agents |
| 6 | Desktop app is not running for a live control command |

Errors are written to stderr as JSON:

```json
{
  "schema": 1,
  "error": {
    "code": "not_in_session",
    "message": "WARDIAN_SESSION_ID environment variable is not set",
    "hint": "Pass a name or uuid to look up a specific agent from outside a Wardian-managed agent process: `wardian agent <name>`.",
    "details": {
      "command": "agent",
      "requested": "self"
    }
  }
}
```

## Related Links

- [Getting Started](./getting-started.md)
- [Watchlists](./watchlists.md)
- [Command Panel](./command-panel.md)
- [Workflows](../workflows/index.md)
- [Native E2E Harness](../developer/native-e2e.md)
