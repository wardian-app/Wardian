# Native E2E Harness

Wardian uses a native Tauri/WebDriver harness for tests that must exercise real IPC, PTY behavior, provider spawning, or terminal rendering. Browser-only Playwright tests are not sufficient evidence for those areas.

## Setup

Run the cross-platform setup command:

```bash
npm run setup:e2e:native
```

The setup script:

- verifies `cargo` is available
- installs `tauri-driver` when missing
- checks for a native WebDriver
- on Windows, can download a matching `msedgedriver.exe` into `tools/e2e-native/`
- on macOS/Linux, prints the WebDriver package or `WARDIAN_NATIVE_WEBDRIVER` guidance when automatic setup is not reliable

The legacy command is kept as an alias:

```bash
npm run setup:e2e:native:windows
```

Generated driver artifacts belong under `tools/e2e-native/` and are ignored by git.

## Running

Run the native mock-provider suite:

```bash
npm run test:e2e:native
```

For rapid iteration after you already have a current native build, reuse the
existing binary instead of rebuilding on every run:

```bash
npm run test:e2e:native:fast
```

Use the fast command only after rebuilding the native binary for Rust or bundled-asset changes:

```bash
npm run tauri -- build --debug --no-bundle
```

You can also target a specific file:

```bash
npm run test:e2e:native:fast -- e2e-native/tests/opencode-native.test.mjs
```

For manual validation, run the same native harness in visible watch mode:

```bash
npm run test:e2e:native:watch -- e2e-native/tests/cli-shared-state-native.test.mjs
```

Watch mode reuses the current native binary, prints named test steps, pauses briefly between watch steps, and keeps the WebView open until you press Enter. Set `WARDIAN_E2E_STEP_DELAY_MS` to change the pause length, or set `WARDIAN_E2E_WATCH_KEEP_OPEN=0` to close the window automatically.

If the WebView shows a `localhost:1420` connection failure, the fast/watch runner is using a binary that expects the Vite dev server. Either start `npm run vite` or rebuild the native debug app first:

```bash
npm run tauri -- build --debug --no-bundle
```

Use this layer when validating:

- terminal scrollback or renderer behavior
- Tauri `invoke` commands
- PTY-backed input/output
- provider spawn, resume, or shutdown behavior
- app and CLI shared-state behavior through isolated `WARDIAN_HOME`
- workflow behavior that depends on native runtime state

The CLI shared-state smoke can be run directly:

```bash
npm run test:e2e:native:fast -- e2e-native/tests/cli-shared-state-native.test.mjs
```

It starts the native app with an isolated `WARDIAN_HOME`, creates agents through both Tauri IPC and live CLI control, then runs the local `wardian-cli` binary against the same home. The smoke asserts live app state is readable, explicit `agent spawn --provider --class` works, `send --wait-until` can drive a mock action-required turn to idle, and lifecycle commands affect the running app. The CLI still falls back to `state.db` when the desktop app is not running.

## Real Providers

Real-provider checks are opt-in. Keep them isolated and only use them when the mock provider cannot prove the behavior.

```bash
WARDIAN_E2E_REAL_OPENCODE=1 WARDIAN_E2E_REAL_WORKSPACE=<absolute-workspace-path> npm run test:e2e:native
```

On PowerShell, use the same placeholder with a Windows absolute path:

```powershell
$env:WARDIAN_E2E_REAL_OPENCODE='1'
$env:WARDIAN_E2E_REAL_WORKSPACE='<absolute-workspace-path>'
npm run test:e2e:native
```

The harness uses an isolated `WARDIAN_HOME` by default, so native E2E runs should not modify production `<wardian-home>` state.

### Real Provider PTY Rendering Lab

Use the rendering lab when investigating issue #110 class failures: line wrapping, row bleed, stale terminal geometry after resize, resize/minimize/maximize lag, or whole-app slowdown with real provider PTYs. This suite is real-provider-only and defaults to Codex and Claude when `WARDIAN_E2E_RENDERING_PROVIDERS` is unset.

POSIX shell:

```bash
export WARDIAN_E2E_REAL_RENDERING=1
export WARDIAN_E2E_RENDERING_PROVIDERS=codex,claude
export WARDIAN_E2E_REAL_WORKSPACE=<absolute-workspace-path>
export WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT=WARDIAN_SCROLL_050
VITE_WARDIAN_TERMINAL_DEBUG=1 npm run tauri -- build --debug --no-bundle
npm run test:e2e:native:fast -- e2e-native/tests/real-provider-rendering-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_RENDERING = '1'
$env:WARDIAN_E2E_RENDERING_PROVIDERS = 'codex,claude'
$env:WARDIAN_E2E_REAL_WORKSPACE = '<absolute-workspace-path>'
$env:WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT = 'WARDIAN_SCROLL_050'
$env:VITE_WARDIAN_TERMINAL_DEBUG = '1'
npm run tauri -- build --debug --no-bundle
npm run test:e2e:native:fast -- e2e-native/tests/real-provider-rendering-native.test.mjs
```

The run writes Wardian-side evidence under `e2e/screenshots/real-provider-rendering/<run-id>/`. Each provider directory includes JSON and screenshots for `initial`, `settled`, `narrow`, `resized`, `wide`, card-level `card-maximized` and `card-restored`, window-level `minimized`, `restored-after-minimize`, `maximized`, `restored-after-maximize`, `rapid-resize-final`, `scrolled-top`, `cleared-immediate`, `paused`, and `resumed`.

When `WARDIAN_E2E_RENDERING_INPUT_TEXT` is unset, the lab submits a compact default prompt that asks the provider to print exactly 50 lines from `WARDIAN_SCROLL_001` through `WARDIAN_SCROLL_050`. This keeps the typed prompt from polluting scrollback with a second copy of the audit rows while still requiring `WARDIAN_SCROLL_050` in the provider response.

Each state JSON records the xterm parser rows, DOM rows, card/screen/viewport rectangles, terminal debug columns and rows, renderer cell metrics, native window rectangle, browser viewport metrics, app-shell rectangle, screenshot timestamps, artifact timestamps, and row-stability timing. Resize and disruptive-action states also record before/after native window rectangles, before/after browser viewport metrics, before/after terminal debug geometry, action duration, stable-row duration, and any exposed fit or resize counters.

The full native runner enables `VITE_WARDIAN_TERMINAL_DEBUG=1` while it builds the app for this lab. `test:e2e:native:fast` intentionally reuses a prebuilt native app, so build the debug app with that Vite flag first; otherwise the Wardian-side terminal debug snapshots will be unavailable.

When terminal history exists, the lab also captures `<state>-scrollback-top` and, for deeper history, `<state>-scrollback-mid` artifacts. These are intentionally not limited to the visible bottom viewport; use them to diagnose row bleed, wrapped-line corruption, stale geometry, and defects that only appear higher in scrollback after resize, card maximize/restore, clear, pause, or resume flows.

Useful tuning variables:

```bash
WARDIAN_E2E_RENDERING_WINDOW_WIDTH=1280
WARDIAN_E2E_RENDERING_WINDOW_HEIGHT=1100
WARDIAN_E2E_RENDERING_RESIZED_WIDTH=980
WARDIAN_E2E_RENDERING_RESIZED_HEIGHT=980
WARDIAN_E2E_RENDERING_WIDE_WIDTH=1440
WARDIAN_E2E_RENDERING_WIDE_HEIGHT=1100
WARDIAN_E2E_RENDERING_RAPID_SEQUENCE=1040x900,1320x1040,1160x980,980x980
WARDIAN_E2E_RENDERING_ROW_HEIGHT=900
WARDIAN_E2E_TERMINAL_FONT_SIZE=10
WARDIAN_E2E_RENDERING_STABLE_ROWS_QUIET_MS=750
WARDIAN_E2E_RENDERING_SETTLE_TIMEOUT_MS=10000
WARDIAN_E2E_RENDERING_POST_INPUT_WAIT_MS=0
WARDIAN_E2E_RENDERING_SUBMIT_INPUT=1
WARDIAN_E2E_RENDERING_SUBMIT_SEQUENCE=\r
WARDIAN_E2E_RENDERING_POST_SUBMIT_WAIT_MS=8000
WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT=<optional-response-marker>
WARDIAN_E2E_RENDERING_CODEX_MODEL=<optional-codex-model>
WARDIAN_E2E_RENDERING_CLAUDE_MODEL=<optional-claude-model>
WARDIAN_E2E_RENDERING_OPENCODE_MODEL=opencode/deepseek-v4-flash-free
```

The lab sends the configured input text as PTY keystrokes and, by default, submits it with carriage return (`\r`). This is intentional: the real-provider run must create actual conversation history before resize, scrollback, clear, pause, and resume evidence is captured. When `WARDIAN_E2E_RENDERING_INPUT_TEXT` is unset, the default prompt asks the provider to print 50 numbered lines from `WARDIAN_SCROLL_001` through `WARDIAN_SCROLL_050`, and the expected response marker defaults to `WARDIAN_SCROLL_050`. For custom deterministic history checks, make the input text ask for a short marker and set `WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT` to that marker. Set `WARDIAN_E2E_RENDERING_SUBMIT_INPUT=0` only when intentionally inspecting prompt-editing behavior without a completed provider turn.

For OpenCode rendering runs, the lab defaults to the free remote OpenCode model `opencode/deepseek-v4-flash-free` so the provider does not fall back to local model backends such as LM Studio. Override `WARDIAN_E2E_RENDERING_OPENCODE_MODEL` only when intentionally testing a different OpenCode model.

The lab fails the run for obvious Wardian-side evidence problems before manual screenshot inspection: non-empty screenshot requirements, missing fixed audit text after resize, unchanged columns when a resize state expects a geometry change, screen rectangle mismatch against xterm cell metrics, paused-buffer mismatch, and rendered rows that do not stabilize before the settle timeout. Outside-terminal parity is still captured separately with `scripts/capture-outside-provider-rendering.ps1` when side-by-side native Windows Terminal evidence is needed.

## Related Research

- [Agent Evaluation References](../research/agent-evaluation-references.md)
