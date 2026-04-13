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

You can also target a specific file:

```bash
npm run test:e2e:native:fast -- e2e-native/tests/opencode-native.test.mjs
```

Use this layer when validating:

- terminal scrollback or renderer behavior
- Tauri `invoke` commands
- PTY-backed input/output
- provider spawn, resume, or shutdown behavior
- workflow behavior that depends on native runtime state

## Real Providers

Real-provider checks are opt-in. Keep them isolated and only use them when the mock provider cannot prove the behavior.

```bash
WARDIAN_E2E_REAL_OPENCODE=1 WARDIAN_E2E_REAL_WORKSPACE=/path/to/workspace npm run test:e2e:native
```

On PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_OPENCODE='1'
$env:WARDIAN_E2E_REAL_WORKSPACE='D:\Development\Wardian'
npm run test:e2e:native
```

The harness uses an isolated `WARDIAN_HOME` by default, so native E2E runs should not modify production `~/.wardian` state.
