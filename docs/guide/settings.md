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

## Related References

- [UI Overview](./ui-overview.md)
- [Provider Runtimes](../providers.md)
- [In-App Updates](../specs/2026-05-19-in-app-updates.md)
- [Runtime Shell Selection](../specs/2026-03-30-runtime-shell-selection.md)
- [Session Persistence Policy](../specs/2026-04-17-session-persistence-policy.md)
