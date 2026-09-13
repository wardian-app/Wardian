# Provider Readiness

Wardian launches provider CLIs that are already installed and authenticated on your machine. Before spawning your first agent, verify at least one supported provider in a normal terminal.

Use this guide when a provider does not appear to start, opens an unexpected sign-in prompt, or works in one shell but not inside Wardian.

## What Wardian Detects

Wardian can detect and launch supported provider commands when they are visible from the Wardian app process environment. For desktop launches, that usually means the provider command must be on the user or system `PATH` before Wardian starts.

The app checks whether the provider executable exists. It does not run provider probes such as `--version`, test account status, validate billing, or check workspace trust. A provider can appear selectable and still fail later if its own runtime setup is incomplete.

In spawn, custom clone, and agent configuration forms, Wardian lists the supported user-facing providers and disables any provider whose CLI command is not found by the app process. Disabled provider options are labeled as not installed. If no supported provider command is found, launch actions are disabled until at least one provider CLI is installed and visible to Wardian.

Wardian does not install provider accounts, complete browser sign-in, create provider billing, or repair shell startup files. Do those steps in a normal terminal before spawning an agent.

## Credential and Session Identity Safety

Wardian keeps its stable agent UUID separate from the provider's conversation identifier. Claude and Pi receive distinct caller-owned provider IDs. Codex creates a distinct local rollout UUID and resumes it exactly, OpenCode binds the exact `ses_...` ID returned by its current run, and Antigravity records only the workspace mapping proven to have changed after the first real user prompt. Structured initialization events can confirm that bound ID, but they cannot replace it.

Before a provider starts, Wardian rejects session identifiers that match credential-bearing environment values such as API keys, tokens, secrets, or passwords. It also rejects provider IDs that equal the Wardian agent UUID, have the wrong provider-specific shape, or conflict with the already-bound ID. Wardian records launch argument counts and resume presence in debug logs instead of raw arguments or provider-supplied identifiers.

Wardian does not guess when an exact provider identity is missing. It does not scan for the newest session, use provider `latest` or `--continue` behavior, substitute the Wardian UUID, or select a session from another workspace. Once a provider identity is known, resume, restore, clone, or clear returns an error rather than replacing that identity. Antigravity is the narrow exception before the first real prompt: it deliberately starts a fresh conversation so it can capture a new mapping, never a guessed one.

If Wardian reports that a session identifier matches a credential environment value, stop the affected agent, rotate the exposed credential if it may have been persisted by an older Wardian version, and launch the agent again. Wardian does not print the matching value in the error.

## Basic Automation

1. Install one provider CLI.
2. Confirm the command is on `PATH`.
3. Run the provider once in a normal terminal and complete its authentication flow.
4. Restart Wardian after changing `PATH` so the app process can see the provider command.
5. Return to [Getting Started](./getting-started.md) and spawn an agent with an enabled provider.

You can choose a preferred launch provider in [Settings](./settings.md). `Auto` keeps the Claude-first default when Claude is installed, then falls back to the first installed supported provider.

## Opt-In Provider Execution Validation

Maintainers can run real-provider automation and delivery matrices after all provider CLIs are installed, authenticated, and trusted for the target workspace. This validation is opt-in because it sends prompts to live provider accounts. Provider-runtime claims must use this real-provider layer or another real-provider native E2E test; mock-provider tests are only valid for Wardian-owned routing, state, queueing, UI, and deterministic terminal plumbing.

The maintained matrix includes Codex, Claude, OpenCode, Antigravity, and Pi. Deprecated Gemini is intentionally excluded.

Run the temporary-provider automation matrix to prove that each provider can launch headlessly, execute an automation task, and return readable node output. The Codex leg deliberately uses an isolated non-Git workspace so the `codex exec --skip-git-repo-check` path is exercised:

```bash
WARDIAN_E2E_REAL_HEADLESS_PROVIDERS=1 \
WARDIAN_E2E_HEADLESS_PROVIDERS=codex,claude,opencode,antigravity,pi \
WARDIAN_E2E_REAL_WORKSPACE="<absolute-workspace-path>" \
npm run test:e2e:native:fast -- e2e-native/tests/provider-headless-automation-real-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_HEADLESS_PROVIDERS = "1"
$env:WARDIAN_E2E_HEADLESS_PROVIDERS = "codex,claude,opencode,antigravity,pi"
$env:WARDIAN_E2E_REAL_WORKSPACE = "<absolute-workspace-path>"
npm run test:e2e:native:fast -- e2e-native/tests/provider-headless-automation-real-native.test.mjs
Remove-Item Env:\WARDIAN_E2E_REAL_HEADLESS_PROVIDERS
Remove-Item Env:\WARDIAN_E2E_HEADLESS_PROVIDERS
Remove-Item Env:\WARDIAN_E2E_REAL_WORKSPACE
```

Run the delivery matrix to verify that each selected provider can launch as an agent and complete the maintained human-composer delivery cases:

```bash
WARDIAN_E2E_REAL_DELIVERY=1 WARDIAN_E2E_DELIVERY_PROVIDERS=codex,claude,opencode,antigravity,pi npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_DELIVERY = "1"
$env:WARDIAN_E2E_DELIVERY_PROVIDERS = "codex,claude,opencode,antigravity,pi"
npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
Remove-Item Env:\WARDIAN_E2E_REAL_DELIVERY
Remove-Item Env:\WARDIAN_E2E_DELIVERY_PROVIDERS
```

The default delivery matrix uses the provider's human composer. To qualify an
existing-session native route, set
`WARDIAN_E2E_DELIVERY_NATIVE_PROVIDERS` to a subset of the selected providers.
The candidate route must establish the provider session first, disable its
owned PTY runtime, preserve the provider session and generation, and complete
one canonical `wardian message followup` with an exact correlated reply. A
missing native capability or delivery fails that candidate; it does not fall
back to the composer or classify the provider as unsupported. Providers that
are not selected as candidates retain the human-composer matrix.

Example Codex candidate run:

```bash
WARDIAN_E2E_REAL_DELIVERY=1 \
WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL=1 \
WARDIAN_E2E_DELIVERY_PROVIDERS=codex \
WARDIAN_E2E_DELIVERY_NATIVE_PROVIDERS=codex \
npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_DELIVERY = "1"
$env:WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL = "1"
$env:WARDIAN_E2E_DELIVERY_PROVIDERS = "codex"
$env:WARDIAN_E2E_DELIVERY_NATIVE_PROVIDERS = "codex"
npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
Remove-Item Env:\WARDIAN_E2E_REAL_DELIVERY
Remove-Item Env:\WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL
Remove-Item Env:\WARDIAN_E2E_DELIVERY_PROVIDERS
Remove-Item Env:\WARDIAN_E2E_DELIVERY_NATIVE_PROVIDERS
```

The delivery test enables Codex workspace trust only inside its isolated
`WARDIAN_HOME`; it does not alter the user's Codex configuration or approval
policy. Authentication and any other provider-specific first-run requirements
must still be completed before running the matrix.

To limit a local run while debugging one provider, set `WARDIAN_E2E_DELIVERY_PROVIDERS` and explicitly allow a partial matrix:

```bash
WARDIAN_E2E_REAL_DELIVERY=1 WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL=1 WARDIAN_E2E_DELIVERY_PROVIDERS=codex,claude npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_DELIVERY = "1"
$env:WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL = "1"
$env:WARDIAN_E2E_DELIVERY_PROVIDERS = "codex,claude"
npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
Remove-Item Env:\WARDIAN_E2E_REAL_DELIVERY
Remove-Item Env:\WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL
Remove-Item Env:\WARDIAN_E2E_DELIVERY_PROVIDERS
```

When either real-provider environment switch is set, unknown provider names fail the corresponding test. The complete five-provider matrix is required unless its matching `*_ALLOW_PARTIAL=1` flag is also set.

By default the real delivery test runs `prompt-short` through the human composer for each selected provider. Use `WARDIAN_E2E_DELIVERY_CASES=all` for the full input case set, or a comma list such as `prompt-short,prompt-multiline`. The maintained case names are `prompt-short`, `prompt-multiline`, `prompt-trailing-newline`, and `prompt-long-paste`.

Use cheap or fast model overrides where the provider exposes a model flag. The delivery test defaults Claude to `haiku` and OpenCode to `opencode/deepseek-v4-flash-free`. Override these with provider-specific environment variables:

```bash
WARDIAN_E2E_DELIVERY_CLAUDE_MODEL=haiku WARDIAN_E2E_REAL_DELIVERY=1 WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL=1 WARDIAN_E2E_DELIVERY_PROVIDERS=claude npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_DELIVERY_CLAUDE_MODEL = "haiku"
$env:WARDIAN_E2E_REAL_DELIVERY = "1"
$env:WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL = "1"
$env:WARDIAN_E2E_DELIVERY_PROVIDERS = "claude"
npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
Remove-Item Env:\WARDIAN_E2E_DELIVERY_CLAUDE_MODEL
Remove-Item Env:\WARDIAN_E2E_REAL_DELIVERY
Remove-Item Env:\WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL
Remove-Item Env:\WARDIAN_E2E_DELIVERY_PROVIDERS
```

## Shared Checks

Most supported providers are distributed through Node.js packages. Check Node and npm first:

```bash
node --version
npm --version
```

Confirm provider commands are visible:

```bash
command -v agy
command -v claude
command -v codex
command -v opencode
command -v pi
```

PowerShell:

```powershell
node --version
npm --version
Get-Command agy, claude, codex, opencode, pi -ErrorAction SilentlyContinue
```

If a command appears only after a shell startup script modifies `PATH`, make that path available to the app process as well. The agent default shell setting controls shell-hosted commands; interactive provider spawning resolves the provider executable before that shell runs.

## Gemini CLI (Deprecated)

Gemini is a legacy provider and is not included in Wardian's maintained real-provider execution matrix. Use Codex, Claude, OpenCode, Antigravity, or Pi for new automations and provider-runtime verification. Antigravity remains a separate provider and uses the `agy` command.

## Antigravity

Install and setup instructions are maintained in the [Antigravity CLI overview](https://www.antigravity.google/docs/cli-overview).

Verify:

```bash
agy --version
agy
```

PowerShell:

```powershell
agy --version
agy
```

Complete Antigravity authentication in the terminal. Wardian checks the `agy` executable before launch, then binds the exact workspace conversation mapping created by that launch in Antigravity's local CLI data. An absent or unchanged mapping is an error; Wardian does not select the newest conversation directory.

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

## Pi

Install:

```bash
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

PowerShell uses the same npm command.

Verify the CLI and configured model catalogue:

```bash
pi --version
pi --list-models
pi
```

Complete model-provider setup in Pi before launching it through Wardian. Pi can
use environment API keys, OAuth-backed providers, or custom models from its
`models.json`. An empty `pi --list-models` result means the executable is ready
but no selectable model is configured.

On Windows, install Git Bash or another Bash-compatible shell supported by Pi.
Pi's project approval prompt controls project-local configuration, extensions,
and skills; it does not sandbox tool or extension execution.

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

If the command exists in a terminal but Wardian still cannot find it, update the user or system `PATH` that the desktop app inherits, then fully restart Wardian. Changing the agent **Default Shell** can help shell-hosted automation commands, but it does not by itself make an interactive provider executable visible to the Wardian app process.

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

Provider shims can behave differently in bash, zsh, PowerShell, cmd, Git Bash, WSL, and package-manager shells. Wardian resolves the interactive provider executable from the app process, then may wrap some Windows shims for compatibility. Use the agent **Default Shell** in [Settings](./settings.md) for shell-hosted commands and automation command nodes, but fix provider-not-found errors by making the provider command visible to the app process `PATH`.

### Provider-Specific Startup Failure

When the command is found but startup still fails:

- Run the provider directly in the target workspace and read its first error.
- Check whether the provider requires project trust, API credits, model access, or a newer Node.js version.
- For OpenCode on Windows, try WSL if the native terminal path fails.
- For deeper runtime differences, compare the provider behavior in [Provider Runtimes](../providers.md).

## Related Links

- [Getting Started](./getting-started.md)
- [Settings](./settings.md)
- [Provider Runtimes](../providers.md)
