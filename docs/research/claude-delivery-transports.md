# Delivering Wardian messages to interactive Claude Code

Research for #1725. Wardian delivers peer messages and task replies to an
interactive Claude Code agent by writing them into the agent's terminal input.
That interrupts the user and can mix with what they are typing. This record
surveys every known way to get a message into a running interactive Claude
session. It weighs the options before choosing one.

Evidence date: 2026-11-12, Claude Code 2.1.322 on Windows 11. Claude Code
documentation is cited by page under `https://code.claude.com/docs/en/`.
"Unverified" means neither the documentation nor a local probe settled the
point.

## Criteria

| Criterion | Why it matters |
| --- | --- |
| Keeps Claude's terminal interface | Hard requirement. Replacing it with Wardian Chat was rejected. |
| Wakes an idle session | Most deliveries arrive while the agent is idle. |
| Leaves the user's draft alone | The defect being fixed. |
| Supported, documented surface | Undocumented internals break silently across releases. |
| Stability | Research-preview contracts may change. |
| Launch friction | Dialogs or flags at every start, plans and organisation policy. |
| Delivery confirmation | Wardian must tell delivered, held and dropped apart without replaying blindly. |
| Turn semantics | Wardian's default is queue until idle. Mid-turn injection must be deliberate. |
| Implementation cost | Work in Wardian. |

## Options

1. **Terminal keystrokes (status quo).** Wakes and queues correctly, but it is
   the defect.
2. **Typing gate.** Defer keystrokes while the composer holds text. This
   mitigates the problem without removing it, and the user rejected it.
3. **SDK route** (`--print --input-format stream-json`). Fully supported, and
   Wardian already has a broker for it. It removes Claude's terminal interface,
   so it was rejected.
4. **Channels** (`channels`, `channels-reference`, research preview). An MCP
   server declares `experimental['claude/channel']` and sends
   `notifications/claude/channel`.
5. **Mod using `$.prompt.submit`** (`plugins/mods/api`). A mod can call
   `$.prompt.submit({ text })`, which "waits until the session is idle and then
   starts a new turn". A mod cannot listen inbound, so it would watch a
   Wardian-owned file or local endpoint using `$.clock.every` with `$.fs.read`
   or `$.http.fetch`. Mods are on by default; install through a plugin
   marketplace or `--plugin-dir`.
6. **Cross-session inbox** (`cross-session-messaging`). Each session binds a
   named pipe (`CLAUDE_CODE_MESSAGING_SOCKET`, with
   `CLAUDE_CODE_MESSAGING_TOKEN`). A hook or Bash command may post to its own
   session's socket, as an "own-child message". Idle sessions start a turn;
   active sessions read the message between tool calls.
7. **`asyncRewake` hook** (`hooks`). A background hook that exits with code 2
   wakes Claude, even when idle. Its stderr arrives as a system reminder.
8. **Stop hook with `decision: "block"`** (`hooks`). The reason becomes input
   and the turn continues. This only works at turn end, and the docs cap it at
   eight consecutive blocks.
9. **`additionalContext`** on UserPromptSubmit or PostToolUse (`hooks`). This
   piggybacks on a request that is already happening and never wakes the
   session.
10. **Background sessions** (`claude --bg`, `claude --resume <id> "prompt"`).
    The prompt reaches a running background session, but the command attaches
    the terminal and refuses piped input or output.
11. **Remote Control** (`remote-control`). Only claude.ai and the mobile apps
    are clients, messages route through Anthropic's servers, and the
    transcript is stored there while connected.
12. **An MCP inbox tool the agent polls.** The agent decides when to look, so
    nothing wakes an idle session.
13. **IDE integration.** Not documented as an input path. Unverified.

## Comparison of the viable options

| | 4. Channels | 5. Mod `prompt.submit` | 6. Cross-session inbox | 7. `asyncRewake` hook |
| --- | --- | --- | --- | --- |
| Keeps the terminal interface | Yes | Yes | Yes | Yes |
| Wakes an idle session | **Yes (probe)**: reply about 2 s after the event | **Yes (probe)**: reply about 2 s after the trigger | Yes (docs) | Yes (docs summary; per-event support unverified) |
| Leaves the draft alone | **Yes (probe)**: draft stayed on screen and was never submitted | **Yes (probe)**: draft stayed on screen and was never submitted | Unverified | Unverified |
| Turn semantics | Queued to the next turn while busy (docs); answered after a running turn (probe) | Waits for idle (docs, **probe**): `submit` resolves when queued, about 30 ms; a submission during a running turn was answered after it | Read between tool calls mid-turn (docs) | Wake only; delivered as a system reminder, not a message |
| Supported surface | Documented, **research preview**; "contract may change" | Documented plugin API, but the API type declarations are marked **"EARLY ACCESS: this surface may change between releases without notice"** | Socket documented for scripts and hooks; **message format undocumented** | Documented hook field |
| Launch friction | A custom server needs `--dangerously-load-development-channels`: a **warning dialog on every launch** (probe). Avoiding it needs an organisation allowlist in managed settings (`allowedChannelPlugins`) | None (probe): `CLAUDE_CODE_PLUGIN_DIRS` in the launch environment loads the mod like `--plugin-dir`, with no install and no dialog | None; on by default since 2.1.234 on Windows | Hook in projected settings |
| Plan and policy limits | claude.ai or Console key; Team and Enterprise must enable | Not documented | `crossSessionInbound` may hold messages for approval depending on the sender's and receiver's permission modes, which matters for bypass agents | None documented |
| Delivery confirmation | No acknowledgement; drops are silent; confirm from the transcript `<channel>` row | The mod writes a queued receipt when `submit` resolves; the transcript row "The <plugin> plugin sent a message:" confirms the turn | No socket acknowledgement documented | Exit status only |
| Wardian cost | Small: channel capability on the existing MCP server, plus launch flag and dialog handling | Small to medium: a Wardian mod, a per-agent queue, and `CLAUDE_CODE_PLUGIN_DIRS` at launch | Small, but relies on an undocumented format | Medium; one-shot, needs re-arming |

Options 8 and 9 can complement another option for mid-turn context. They
cannot be the transport, because neither wakes an idle session.

## Probe record

The channels probe ran a throwaway interactive session in a real pseudo
terminal from a scratch folder. It used `--strict-mcp-config`,
`--setting-sources project`, Haiku, and a minimal stdio MCP server declaring
`claude/channel`. Results:

- **Launch:** one full-screen dialog, "WARNING: Loading development channels",
  defaulting to "I am using this for local development".
- **Idle event:** a turn started and the reply arrived within about 2 seconds.
  The transcript shows `<channel source="wardianspike" sender="spike" trigger="01_txt">`.
- **Event while the user had a draft:** answered; the draft stayed in the
  prompt box and was never submitted.
- **Event during a running turn:** answered after the turn.

The mod probe used the same harness without channels. A mod loaded with
`--plugin-dir` polls a trigger folder every second through `$.clock.every`,
`$.fs.list` and `$.fs.read`, calls `$.prompt.submit({ text })`, and writes
receipts with `$.fs.write`. Results:

- **Launch:** no dialog.
- **Idle trigger:** a turn started and the reply arrived within about 2 seconds.
  The transcript shows a user row beginning "The wardianmodprobe plugin sent a
  message:", followed by an engine note that the prompt "starts this turn in
  the user's place".
- **Trigger while the user had a draft:** answered; the draft stayed in the
  prompt box and was never submitted.
- **Trigger during a running turn:** `submit` resolved in about 30 ms, and the
  turn it queued ran after the running turn ended.
- **Caveat:** in one run an unrelated Claude Code onboarding dialog ("Teach
  auto mode about your environment?") opened and captured the probe's typed
  draft. The rerun, which dismissed dialogs before typing, passed every check.
  A plugin submission was still delivered while that dialog was open.

## Assessment

A **mod using `$.prompt.submit`** is recommended. It is the only option that
meets every criterion:

- it keeps Claude's terminal interface;
- it wakes an idle session;
- it leaves the user's draft alone;
- it queues until idle, which is Wardian's own default;
- it is a documented plugin API (marked early access, like channels' research preview);
- it needs no flag or dialog, because Wardian projects it with
  `CLAUDE_CODE_PLUGIN_DIRS` at launch;
- it gives Wardian a queued receipt plus a distinctive transcript row for
  confirmation.

Remaining design points belong to the spec, not this survey:

- how Wardian hands a message to the mod (a per-agent queue directory or a
  local endpoint);
- how sender identity and reply instructions are framed in the prompt text;
- what happens when the mod is not loaded (fall back to the current path and
  report why);
- how this interacts with Claude's own cross-session inbox.

**Channels** is the fallback. It is proven by probe, but it costs a warning
dialog on every launch without an organisation allowlist. Both channels and
mods are pre-stable surfaces, so stability does not separate them; the
launch dialog does. The **cross-session inbox** is not recommended: its
message format is undocumented, and its hold rules interact with
bypass-permission agents.
