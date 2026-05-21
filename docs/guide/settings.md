# Settings

Settings controls Wardian's global app preferences, display behavior, terminal
defaults, provider defaults, and provider maintenance utilities.

Open Settings from the gear icon on the left icon rail. It opens as a
near-full-screen app modal and does not change the currently selected sidebar
pane or main workspace view.

![Wardian Settings modal showing agent runtime defaults and concise setting descriptions](../assets/screenshots/settings/runtime-settings.png)

## Storage

Settings in this screen are global. Wardian stores durable settings under
`<WARDIAN_HOME>/settings/`:

- `settings/app.json`: app preferences such as theme, terminal font size,
  terminal font family, and Gemini auto-patch.
- `settings/shell.json`: runtime preferences such as shell selection, default
  provider, regular agent session policy, and Codex runtime defaults.

Project, workspace, agent-class, and per-agent settings scopes are not part of
the current Settings screen.

## Navigation and Search

The Settings modal has category navigation and a search field. Search matches
setting labels, short descriptions, and related keywords.

Categories:

- **General**: app version and update status.
- **Appearance**: app theme.
- **Terminal**: terminal font and shell defaults.
- **Agent Runtime**: default provider, regular agent session behavior, and
  provider-specific runtime defaults such as the Codex subsection.
- **Provider Utilities**: provider-specific maintenance actions such as Gemini
  patching.
- **Advanced**: settings file and diagnostics information.

Each row includes a short detail line. For example, the default provider row
notes that **Auto** prefers Claude when available.

## Updates

The General category shows the currently running Wardian version, such as
`Wardian v0.3.6`.

Wardian checks for updates silently when Settings loads. If no newer stable
release is available, Settings shows that Wardian is up to date. If a newer
release is available, use **Download & Install** to fetch and install it from
inside the app.

Wardian does not install updates or restart automatically. After installation
completes, use **Restart** to relaunch into the updated version.

Update checks are available only in official installed release builds. Dev
builds and local source-built binaries show the running Wardian version, but
Settings disables update checks so local builds are not replaced by public
release installers.

## Appearance

Theme options:

- **System**
- **Dark**
- **Light**

Wardian applies the selected mode to the app UI and syncs the OpenCode theme
preference through backend settings.

## Terminal

Terminal settings control the embedded terminal display and the shell used for
shell-hosted commands:

- **Terminal font size** applies immediately to embedded agent terminals.
- **Terminal font family** can use the platform default or a selected monospace
  font. The default option names the font Wardian currently resolves.
- **Integrated terminal shell** can use Wardian's resolved default shell, a
  discovered shell, or Custom. The default option names the shell Wardian
  currently resolves.

Runtime shell changes affect future terminal launches and shell-hosted workflow
commands.

## Agent Runtime

**Default provider** controls the provider Wardian preselects when starting a
new visible agent. **Auto** prefers Claude when available and falls back to the
first installed supported provider.

**Regular agent sessions** controls how normal visible agents behave when
resumed from `Off`:

- **Resume sessions**
- **Start fresh**

Workflow Agent nodes use their own node-level run mode and do not inherit this
global regular-agent setting.

The **Codex** subsection contains Codex-specific runtime defaults. These apply
when Codex agents do not set explicit advanced sandbox or approval overrides.

## Provider Utilities

The Gemini patch controls help Gemini discover Wardian skills:

- **Auto-patch Gemini CLI**
- **Run Patch Now**

Changing Gemini patch settings does not affect Claude, Codex, Antigravity, or
OpenCode behavior.

## Troubleshooting

- If shell options are empty, verify shell binaries are installed and visible in
  your OS PATH.
- If provider options are disabled elsewhere in Wardian, install the provider
  CLI and make sure Wardian can see it on the app process PATH. See
  [Provider Readiness](./provider-readiness.md).
- If resume behavior is not what you expect, verify both the global runtime
  policy and any per-agent override.
- If Gemini skills are missing, run the patch manually and restart the app
  session.

## Important Limits

- Settings are global only in the current implementation.
- Shell and runtime policy changes affect future launches and resumes; they do
  not reconfigure an already-running provider process.
- Workflow Agent nodes have their own execution mode and do not simply inherit
  the regular-agent resume default.
- Provider utilities are provider-specific.

## Related Links

- [UI Overview](./ui-overview.md)
- [Getting Started](./getting-started.md)
- [Provider Readiness](./provider-readiness.md)
- [Provider Runtimes](../providers.md)
