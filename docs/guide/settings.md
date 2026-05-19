# Settings

The Settings tab controls app updates, theme behavior, runtime shell selection, session policy defaults, and Gemini patch utilities.

![Wardian Settings tab showing theme, session persistence, and shell selection controls](../assets/screenshots/settings/runtime-settings.png)

## Updates

The top of Settings shows the currently running Wardian version, such as `Wardian v0.3.5`.

Wardian checks for updates silently when Settings loads. If no newer stable release is available, Settings shows that Wardian is up to date. If a newer release is available, use **Download & Install** to fetch and install it from inside the app.

Wardian does not install updates or restart automatically. After installation completes, use **Restart** to relaunch into the updated version.

Update checks are available only in official installed release builds. Dev builds and local source-built binaries still show the running Wardian version, but Settings disables update checks so local builds are not replaced by public release installers.

If update checks fail:

- verify internet access to GitHub Releases
- try **Check Now**
- install the latest release manually if the running build predates in-app update support

## Theme

Theme options:

- **System**
- **Dark**
- **Light**

Wardian applies the selected mode to the app UI and syncs the OpenCode theme preference through backend settings.

## Agent Runtime: Default Provider

The **Default provider** control sets the provider Wardian preselects when starting a new visible agent:

- **Auto** prefers Claude when the Claude CLI is installed, then uses the first installed supported provider.
- **Claude**, **Codex**, **Gemini**, and **OpenCode** select that provider when its CLI is installed.

If an explicit default provider is not installed, Wardian falls back to the first installed provider and shows a note in the spawn form. Provider options whose CLI command is not found are disabled in launch forms.

After changing the default provider, use **Save Agent Runtime**.

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
- If provider options are disabled, install the provider CLI and make sure Wardian can see it on the app process PATH. See [Provider Readiness](./provider-readiness.md).
- If resume behavior is not what you expect, verify both the global runtime policy and any per-agent override.
- If Gemini skills are missing, run patch manually and restart the app session.

## Related References

- [UI Overview](./ui-overview.md)
- [Provider Readiness](./provider-readiness.md)
- [Provider Runtimes](../providers.md)
