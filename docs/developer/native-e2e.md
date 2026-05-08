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
WARDIAN_E2E_REAL_OPENCODE=1 WARDIAN_E2E_REAL_WORKSPACE=/path/to/workspace npm run test:e2e:native
```

On PowerShell, use the same placeholder with a Windows absolute path:

```powershell
$env:WARDIAN_E2E_REAL_OPENCODE='1'
$env:WARDIAN_E2E_REAL_WORKSPACE='<absolute-workspace-path>'
npm run test:e2e:native
```

The harness uses an isolated `WARDIAN_HOME` by default, so native E2E runs should not modify production `~/.wardian` state.
