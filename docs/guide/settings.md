# Settings

The Settings tab controls theme behavior, runtime shell selection, session policy defaults, and Gemini patch utilities.

Use it when you need to change runtime defaults before spawning more agents, tune provider behavior, or align Wardian with your preferred shell environment.

![Wardian Settings tab showing theme, session persistence, and shell selection controls](../assets/screenshots/settings/runtime-settings.png)

## When to Use It

- Choose the shell that hosts agent providers and shell-based workflow commands.
- Decide whether regular agents resume provider sessions or start fresh after being off.
- Configure provider-specific runtime utilities such as Gemini patching or Codex runtime policy.
- Change the app theme.

## Basic Workflow

1. Open **Settings** from the left control rail.
2. Adjust theme, agent runtime, shell, or provider utility settings.
3. Use the relevant save button for shell or runtime changes.
4. Spawn or resume an agent to confirm the new runtime behavior.
5. Check [Provider Runtimes](../providers.md) when behavior differs by CLI provider.

## Theme

Theme options:

- **System**
- **Dark**
- **Light**

Wardian applies the selected mode to the app UI and syncs the OpenCode theme preference through backend settings.

## Agent Runtime: Regular Agent Sessions

This setting controls how normal visible agents behave when resumed from `Off`:

- **Resume provider session** (`resume`)
- **Start fresh on resume** (`fresh`)

Important:

- This is a global default for regular agents.
- Workflow Agent nodes use their own node-level run mode and do not inherit this global setting.

## Default Shell

You can choose:

- **Auto**
- a discovered shell from your system
- **Custom** executable + args

Typical uses:

- force PowerShell, bash, zsh, or cmd as host shell
- point to a custom shell path for team-standard environments

After changing shell settings, use **Save Shell**.

## Advanced: Gemini Patch

Controls:

- **Auto-patch Gemini CLI** toggle
- **Run Patch Now** button

Use this when Gemini skill discovery requires Wardian's patch flow.

## Troubleshooting

- If shell options are empty, verify shell binaries are installed and visible in your OS PATH.
- If resume behavior is not what you expect, verify both the global runtime policy and any per-agent override.
- If Gemini skills are missing, run patch manually and restart the app session.

## Important Limits

- Shell and runtime policy changes affect future launches and resumes; they do not reconfigure an already-running provider process.
- Workflow Agent nodes have their own execution mode and do not simply inherit the regular-agent resume default.
- Provider utilities are provider-specific. Changing Gemini patch settings does not affect Claude, Codex, or OpenCode.

## Related Links

- [UI Overview](./ui-overview.md)
- [Getting Started](./getting-started.md)
- [Provider Runtimes](../providers.md)
- [Runtime Shell Selection](../specs/2026-03-30-runtime-shell-selection.md)
- [Session Persistence Policy](../specs/2026-04-17-session-persistence-policy.md)
