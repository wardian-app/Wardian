# Provider Readiness

Wardian launches provider CLIs that are already installed and authenticated on your machine. Before spawning your first agent, verify at least one supported provider in a normal terminal.

Use this guide when a provider does not appear to start, opens an unexpected sign-in prompt, or works in one shell but not inside Wardian.

## What Wardian Detects

Wardian can detect and launch supported provider commands when they are visible from the Wardian app process environment. For desktop launches, that usually means the provider command must be on the user or system `PATH` before Wardian starts.

The app checks whether the provider executable exists. It does not run provider probes such as `--version`, test account status, validate billing, or check workspace trust. A provider can appear selectable and still fail later if its own runtime setup is incomplete.

In spawn, custom clone, and agent configuration forms, Wardian lists the supported user-facing providers and disables any provider whose CLI command is not found by the app process. Disabled provider options are labeled as not installed. If no supported provider command is found, launch actions are disabled until at least one provider CLI is installed and visible to Wardian.

Wardian does not install provider accounts, complete browser sign-in, create provider billing, or repair shell startup files. Do those steps in a normal terminal before spawning an agent.

## Basic Workflow

1. Install one provider CLI.
2. Confirm the command is on `PATH`.
3. Run the provider once in a normal terminal and complete its authentication flow.
4. Restart Wardian after changing `PATH` so the app process can see the provider command.
5. Return to [Getting Started](./getting-started.md) and spawn an agent with an enabled provider.

You can choose a preferred launch provider in [Settings](./settings.md). `Auto` keeps the Claude-first default when Claude is installed, then falls back to the first installed supported provider.

## Shared Checks

Most supported providers are distributed through Node.js packages. Check Node and npm first:

```bash
node --version
npm --version
```

Confirm provider commands are visible:

```bash
command -v gemini
command -v claude
command -v codex
command -v opencode
```

PowerShell:

```powershell
node --version
npm --version
Get-Command gemini, claude, codex, opencode -ErrorAction SilentlyContinue
```

If a command appears only after a shell startup script modifies `PATH`, make that path available to the app process as well. The agent default shell setting controls shell-hosted commands; interactive provider spawning resolves the provider executable before that shell runs.

## Gemini CLI

Install:

```bash
npm install -g @google/gemini-cli
```

Verify:

```bash
gemini --version
gemini
```

Complete the Gemini CLI sign-in or API-key flow in the terminal. After it reaches the interactive prompt, exit and spawn a Gemini agent from Wardian.

## Claude Code

Install:

```bash
npm install -g @anthropic-ai/claude-code
```

Verify:

```bash
claude --version
claude
```

Complete Claude Code authentication in the terminal. If Claude opens a browser or prompts for a plan/account, finish that setup before using Wardian.

## Codex

Install:

```bash
npm install -g @openai/codex
```

Verify:

```bash
codex --version
codex
```

Complete the OpenAI sign-in or credential setup requested by Codex. If Codex asks to trust a workspace, answer that prompt in a normal terminal for the workspace you plan to use.

## OpenCode

OpenCode's official install script is the simplest POSIX path:

```bash
curl -fsSL https://opencode.ai/install | bash
```

The Node.js package is also available:

```bash
npm install -g opencode-ai
```

Verify:

```bash
opencode --version
opencode
```

In the OpenCode TUI, run `/connect` and configure the LLM provider you want OpenCode to use. On Windows, OpenCode's own documentation recommends WSL for the best terminal compatibility; npm, Chocolatey, Scoop, and binary installs are also available.

## Troubleshooting

### Provider Not Found

If Wardian reports that a provider command is missing, first check whether a newly opened terminal can see the command:

```bash
command -v <provider-command>
```

PowerShell:

```powershell
Get-Command <provider-command>
```

If the command exists in a terminal but Wardian still cannot find it, update the user or system `PATH` that the desktop app inherits, then fully restart Wardian. Changing the agent **Default Shell** can help shell-hosted workflow commands, but it does not by itself make an interactive provider executable visible to the Wardian app process.

If the provider is disabled in Wardian but appears in a terminal, the desktop app and that terminal are seeing different environments. Fix the app-level `PATH`, restart Wardian, and check the provider list again.

### Authentication Prompt Appears in the Agent Terminal

Stop the agent, open a normal terminal, and run the provider command directly. Complete browser sign-in, device-code login, API-key entry, billing setup, workspace trust, or provider-specific first-run prompts there. Then spawn the Wardian agent again.

### PATH Mismatch

Global npm installs often land in a user-level bin directory. If your login shell adds that directory but the desktop app was launched before the change, Wardian may not see the provider command.

Prefer one of these fixes:

- Install the provider in a location already visible to all shells you use.
- Add the package-manager bin directory to the user or system `PATH`.
- Fully restart Wardian after changing `PATH`.
- Verify the command from a newly opened terminal that did not inherit a temporary one-session path edit.

### Shell Mismatch

Provider shims can behave differently in bash, zsh, PowerShell, cmd, Git Bash, WSL, and package-manager shells. Wardian resolves the interactive provider executable from the app process, then may wrap some Windows shims for compatibility. Use the agent **Default Shell** in [Settings](./settings.md) for shell-hosted commands and workflow command nodes, but fix provider-not-found errors by making the provider command visible to the app process `PATH`.

### Provider-Specific Startup Failure

When the command is found but startup still fails:

- Run the provider directly in the target workspace and read its first error.
- Check whether the provider requires project trust, API credits, model access, or a newer Node.js version.
- For OpenCode on Windows, try WSL if the native terminal path fails.
- For Gemini skill discovery issues, run the Gemini patch from Settings before changing project files.
- For deeper runtime differences, compare the provider behavior in [Provider Runtimes](../providers.md).

## Related Links

- [Getting Started](./getting-started.md)
- [Settings](./settings.md)
- [Provider Runtimes](../providers.md)
