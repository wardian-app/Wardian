# First-Run Troubleshooting

Use this guide when the first setup path breaks before you can complete a simple agent task. Start with the section that matches what you see, then collect the requested evidence before filing an issue.

## Quick Recovery Order

1. Restart Wardian after installing a provider CLI, changing PATH, or changing shell settings.
2. Open a normal terminal and confirm the provider command starts there.
3. Spawn one agent in a workspace you can open and edit yourself.
4. Watch the agent terminal before changing settings. Authentication, permission prompts, and provider errors usually appear there first.
5. Capture the exact warning or error text before retrying.

## App Launch Warnings

Operating systems may warn before opening a new or unsigned Wardian build.

- **Windows SmartScreen or reputation warnings**: continue only if the installer came from the official Wardian release you intended to install.
- **macOS unidentified developer warnings**: verify the app source before approving the app in system privacy or security settings.
- **Linux executable or package warnings**: verify the package source and file permissions before launching it.

Do not bypass an operating system warning for a build whose source you cannot verify. If a managed device blocks the app, ask your administrator whether local unsigned developer tools are allowed.

If Wardian opens to a blank or broken window, close it completely, reopen it once, and capture the visible warning text or blank-window screenshot if the problem repeats.

## Provider Is Not Detected

Wardian can only spawn an agent for a provider command the app process can find.

Check only the provider command you selected in Wardian. These examples show the supported command names:

```bash
command -v gemini
command -v claude
command -v codex
command -v opencode
```

PowerShell:

```powershell
Get-Command gemini
Get-Command claude
Get-Command codex
Get-Command opencode
```

If the command is missing, install or repair that provider CLI, then restart Wardian. If the command works in a terminal but not in Wardian, restart the desktop app so it receives the updated PATH from the operating system.

If the provider opens but asks you to sign in, finish authentication in the normal terminal first. Then spawn a new Wardian agent with that provider.

## Terminal Does Not Start

If the agent appears but the terminal stays empty, exits immediately, or turns red:

1. Confirm the workspace folder exists and your user account can write to it.
2. Run the selected provider directly in that same workspace.
3. Restart Wardian after installing provider CLIs or shells.
4. Check **Settings** if shell-hosted commands fail. Use **Auto** first unless you need a specific shell.
5. Delete the failed first-run agent and spawn a new one after fixing the provider or workspace issue.

The provider terminal is the first place to inspect. It may show authentication prompts, permission prompts, missing command errors, or provider-specific startup messages that the roster can only summarize as `Error`, `Off`, or `Action Required`.

## Frozen, Off, or Action-Required Agents

Use the status color and the terminal together:

- **Action Required** means the provider likely needs input, approval, or authentication. Click the agent terminal and read the current prompt before restarting it.
- **Off** means Wardian is not currently running that session. Use restart or resume from the roster, then inspect the terminal if it turns off again.
- **Processing** for a long time can be normal during model work, package installs, or large file reads. If there is no terminal output for several minutes, capture the terminal state before restarting.
- **Error** means startup or runtime failed. Capture the visible message and the provider you selected.

Avoid changing multiple settings at once while debugging a first-run agent. Change one thing, restart Wardian if the change affects PATH or shell discovery, and spawn a fresh test agent.

## Queue Looks Empty

The Queue is a completion review surface. It is not a live prompt inbox and it does not show every terminal line.

Queue items appear when:

- an active agent returns to Idle after work
- a workflow run finishes or fails

If the Queue is empty after spawning an agent, that can be expected. Send the agent a small task, wait for it to settle back to Idle, then open **Queue** again. Provider approval prompts still appear through the agent status and terminal rather than as Queue items.

## Wardian CLI Is Not Visible

Wardian installs the `wardian` CLI when the desktop app starts. If a normal terminal cannot find it, restart that terminal after opening Wardian once.

Check from a normal terminal:

```bash
wardian agent list --scope all --fields name,status
```

PowerShell:

```powershell
wardian agent list --scope all --fields name,status
```

If the command works but shows stale data, confirm the Wardian desktop app is running. Live-control commands such as `send`, `spawn`, `pause`, `resume`, and `workflow run` require the app to be running.

## Advanced: Inspect Raw Terminal Output

Most users should start with the visible terminal and the readable CLI output. Raw terminal output and custom Wardian homes are useful only when debugging terminal rendering, escape sequences, PTY transport behavior, or intentionally isolated state.

If you intentionally set a custom `WARDIAN_HOME`, use the same value before starting Wardian and before running CLI commands. Otherwise the app and terminal may read different Wardian homes. Mention this in an issue report without sharing private paths.

Readable snapshot:

```bash
wardian agent watch <agent-name> --include transcript,output
```

PowerShell:

```powershell
wardian agent watch <agent-name> --include transcript,output
```

Raw terminal evidence:

```bash
wardian agent watch <agent-name> --include raw_output --raw
```

PowerShell:

```powershell
wardian agent watch <agent-name> --include raw_output --raw
```

Raw output can contain escape sequences, screen repaint fragments, prompts, file paths, and pasted text. Sanitize secrets before sharing it.

## Filing an Issue

File an issue when the same first-run failure repeats after you verify the provider starts in a normal terminal and restart Wardian.

Include:

- operating system and version
- Wardian version or release name
- provider name and provider CLI version, if available
- selected shell or **Auto**
- whether the provider starts successfully outside Wardian
- the step that failed: launch, provider detection, spawn, terminal startup, agent status, Queue, or CLI
- sanitized terminal output, screenshots, or logs

Do not include API keys, provider tokens, private prompts, private repository contents, `.env` files, or unsanitized terminal logs.

## Related Docs

- [Getting Started](./getting-started.md)
- [Wardian CLI](./cli.md)
- [Queue](./queue.md)
- [Settings](./settings.md)
- [Provider Runtimes](../providers.md)
