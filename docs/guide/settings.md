# Settings

The Settings tab controls theme behavior, runtime shell selection, session policy defaults, and provider-specific runtime utilities.

![Wardian Settings tab showing theme, session persistence, and shell selection controls](../assets/screenshots/settings/runtime-settings.png)

## Theme

Theme options:

- **System**
- **Dark**
- **Light**

Wardian applies the selected mode to the app UI. For OpenCode agents, Wardian also syncs the provider theme preference through backend settings.

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

## Provider Runtime Utilities

Provider-specific settings apply only to providers that need them:

- **Gemini**: skill discovery patch controls.
- **Codex**: runtime policy controls for sandbox mode, approval policy, and full-auto behavior.
- **OpenCode**: theme preference sync.
- **Claude**: permission and session behavior are configured per agent rather than from this global settings panel.

## Advanced: Gemini Patch

Controls:

- **Auto-patch Gemini CLI** toggle
- **Run Patch Now** button

Use this when Gemini skill discovery requires Wardian's patch flow. Other providers use their own discovery paths; see [Provider Runtimes](../providers.md) for the full comparison.

## Troubleshooting

- If shell options are empty, verify shell binaries are installed and visible in your OS PATH.
- If resume behavior is not what you expect, verify both the global runtime policy and any per-agent override.
- If Gemini skills are missing, run the Gemini patch manually and restart the app session.
- If OpenCode skills or instructions are missing, inspect the generated provider config and config directory described in Provider Runtimes.
- If Codex skills are missing, inspect the agent habitat skill projection before changing repository files.

## Related References

- [UI Overview](./ui-overview.md)
- [Provider Runtimes](../providers.md)
- [Spec 010: Runtime Shell Selection](../specs/010-runtime-shell-selection.md)
- [Spec 019: Session Persistence Policy](../specs/019-session-persistence-policy.md)
