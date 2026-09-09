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

Native artifact selection is fail-closed. Cargo metadata supplies the effective
target directory, including `CARGO_TARGET_DIR` and Cargo config overrides; a
stale repository-local `target/` binary is not selected. Set
`WARDIAN_NATIVE_APP=<artifact-path>` only when using a deliberate custom app
artifact. Relative values are resolved from the repository root, and a missing,
empty, or directory override stops before WebDriver/provider startup. The
setup script also verifies package-local `@tauri-apps/cli` and
`selenium-webdriver` resolution before driver setup.

POSIX shell:

```bash
WARDIAN_NATIVE_APP=<artifact-path> npm run test:e2e:native:fast -- <native-test-file>
```

PowerShell:

```powershell
$env:WARDIAN_NATIVE_APP = '<artifact-path>'
npm run test:e2e:native:fast -- <native-test-file>
```

`e2e-native/tests/artifact-presentation-native.test.mjs` proves the artifact
control path with real IPC: an isolated mock agent presents an authorized
Markdown file through the built CLI, Wardian routes a non-focused Files tab,
the renderer opens the canonical file, and the same artifact restores after a
native app relaunch.

## Running

Run the native mock-provider suite:

```bash
npm run test:e2e:native
```

On Windows, the harness launches the installed Tauri CLI through Node for
builds. This preserves the inline build configuration when a test is launched
directly with Node, without npm's environment variables. The test runner also
passes test paths directly to Node, including paths containing spaces.

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

### Concurrent runs

Two native runs can execute at the same time. Every run claims its own
resources automatically, so nothing has to be chosen by hand:

- **Ports.** The driver and its child native driver each get a port reserved
  from the OS at startup, passed through as `--port` and `--native-port`. There
  is no fixed port any more, so a second run cannot collide with the first.
- **Home.** Each run gets `wardian-e2e-native-<runId>` under the OS temp
  directory. The runner pins that value once and hands it to every child, so
  the runner and harness never disagree about which home is in play.
- **Cleanup.** A run ends only the process tree it started. It does not search
  for processes by command line, so an unrelated process is never terminated
  because its command line happens to mention the home path.

Both `npm run test:e2e:native` and the runner script activate this. Nothing
needs a free port picked in advance.

#### Using an explicit home

Set `WARDIAN_E2E_NATIVE_HOME` to keep a run's state for inspection:

```bash
WARDIAN_E2E_NATIVE_HOME=/tmp/wardian-e2e-native-inspect npm run test:e2e:native
```

PowerShell:

```powershell
$env:WARDIAN_E2E_NATIVE_HOME = "$env:TEMP\wardian-e2e-native-inspect"
npm run test:e2e:native
```

The path must be under the OS temp directory and begin with
`wardian-e2e-native`, or sit under `.tmp/e2e-native` in the repository. The
harness resets the home it is given, and that guard is what stops a reset from
reaching an unrelated directory.

A run writes `.native-e2e-lock.json` into its home and removes it on exit. A
second run pointed at the same explicit home is refused before anything is
deleted or terminated. Give each concurrent run its own home, or leave the
variable unset.

If a previous run crashed, its lock is left behind. The next run reports the
stale lock and proceeds. It does not terminate processes that run may have
orphaned, because they cannot be told apart from unrelated processes without
the kind of command-line matching that caused cross-run kills.

#### Endpoint ownership

A run refuses to use a driver endpoint it cannot prove it owns. After the port
answers, the harness resolves the pid listening on it and requires that pid to
be the driver it started or one of that driver's children. A live listener left
by something else fails the run instead of being adopted, and a driver that
exits before binding is reported rather than treated as ready.

Some capture and chat helper scripts still assume the old fixed port. They read
the port from the harness session instead: `harness.driverPort` and
`harness.nativeDriverPort`.

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

- workbench JSON persistence, backup rotation, restart recovery, or safe mode
- terminal scrollback or renderer behavior
- Tauri `invoke` commands
- PTY-backed input/output
- desktop/remote presentation ownership, geometry, snapshots, or event order
- provider spawn, resume, or shutdown behavior
- app and CLI shared-state behavior through isolated `WARDIAN_HOME`
- automation behavior that depends on native runtime state

The CLI shared-state smoke can be run directly:

```bash
npm run test:e2e:native:fast -- e2e-native/tests/cli-shared-state-native.test.mjs
```

It starts the native app with an isolated `WARDIAN_HOME`, creates agents through both Tauri IPC and live CLI control, then runs the local `wardian-cli` binary against the same home. The smoke asserts live app state is readable, explicit `agent spawn --provider --class` works, `send --wait-until` can drive a mock action-required turn to idle, and lifecycle commands affect the running app. The CLI still falls back to `state.db` when the desktop app is not running.

### Workbench and Terminal Presentation Package

After building current native assets, run the focused package:

```bash
npm run tauri -- build --debug --no-bundle
npm run test:e2e:native:workbench
```

It proves four native boundaries with an isolated Wardian home:

- exact workbench primary/backup bytes, restart, corrupt-file fallback, and
  future-schema preservation;
- desktop terminal owner/mirror activation races, timeouts, stale lease
  rejection, ordered output, and stable canonical geometry;
- closing all workbench presentations leaves the agent runtime live, while
  safe mode preserves the durable split tree byte-for-byte;
- authenticated desktop-to-remote-to-desktop transfer and socket liveness.

The related browser package remains useful for navigation and layout only:

```bash
npm run test:e2e:workbench
```

Do not use its mocked transport to claim native file durability, PTY resize,
lease enforcement, remote authentication, or provider behavior.

## Real Providers

Real-provider checks are opt-in. Keep them isolated and use them for every provider-runtime claim. The mock provider can prove Wardian-owned behavior such as routing, queueing, rendering, state sharing, and deterministic PTY plumbing. It cannot prove that a real provider CLI accepts input, exposes a ready prompt, clears a compose field, resumes a session, or responds through its real transcript path.

Never make a mock-backed test spoof a real provider identity to validate Codex, Claude, Gemini, OpenCode, Antigravity, or Pi behavior. If a test would need that, write an opt-in real-provider native E2E test or leave a skipped test with `// @real-provider-only`.

```bash
WARDIAN_E2E_REAL_OPENCODE=1 WARDIAN_E2E_REAL_WORKSPACE=<absolute-workspace-path> npm run test:e2e:native
```

For the opt-in Antigravity smoke:

```bash
WARDIAN_E2E_REAL_ANTIGRAVITY=1 WARDIAN_E2E_REAL_WORKSPACE=<absolute-workspace-path> npm run test:e2e:native:fast -- e2e-native/tests/antigravity-native.test.mjs
```

On PowerShell, use the same placeholder with a Windows absolute path:

```powershell
$env:WARDIAN_E2E_REAL_OPENCODE='1'
$env:WARDIAN_E2E_REAL_WORKSPACE='<absolute-workspace-path>'
npm run test:e2e:native
```

PowerShell Antigravity smoke:

```powershell
$env:WARDIAN_E2E_REAL_ANTIGRAVITY='1'
$env:WARDIAN_E2E_REAL_WORKSPACE='<absolute-workspace-path>'
npm run test:e2e:native:fast -- e2e-native/tests/antigravity-native.test.mjs
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

The provider list also accepts `pi`. Include it in
`WARDIAN_E2E_RENDERING_PROVIDERS` for a Pi rendering run, and set
`WARDIAN_E2E_RENDERING_PI_MODEL` when the test should use a specific configured
Pi model.

The run writes Wardian-side evidence under `e2e/screenshots/real-provider-rendering/<run-id>/`. Each provider directory includes JSON and screenshots for `initial`, `settled`, `narrow`, `resized`, `wide`, card-level `card-maximized` and `card-restored`, window-level `minimized`, `restored-after-minimize`, `maximized`, `restored-after-maximize`, `rapid-resize-final`, `scrolled-top`, `cleared-immediate`, `paused`, and `resumed`.

When `WARDIAN_E2E_RENDERING_INPUT_TEXT` is unset, the lab submits a compact default prompt that asks the provider to print exactly 50 lines from `WARDIAN_SCROLL_001` through `WARDIAN_SCROLL_050`. This keeps the typed prompt from polluting scrollback with a second copy of the audit rows while still requiring `WARDIAN_SCROLL_050` in the provider response.

Each state JSON records the xterm parser rows, DOM rows, card/screen/viewport rectangles, terminal debug columns and rows, renderer cell metrics, native window rectangle, browser viewport metrics, app-shell rectangle, screenshot timestamps, artifact timestamps, and row-stability timing. Resize and disruptive-action states also record before/after native window rectangles, before/after browser viewport metrics, before/after terminal debug geometry, action duration, stable-row duration, and any exposed fit or resize counters.

The full native runner enables `VITE_WARDIAN_TERMINAL_DEBUG=1` while it builds the app for this lab. `test:e2e:native:fast` intentionally reuses a prebuilt native app, so build the debug app with that Vite flag first; otherwise the Wardian-side terminal debug snapshots will be unavailable.

When terminal history exists, the lab also captures `<state>-scrollback-top` and, for deeper history, `<state>-scrollback-mid` artifacts. These are intentionally not limited to the visible bottom viewport; use them to diagnose row bleed, wrapped-line corruption, stale geometry, and defects that only appear higher in scrollback after resize, card maximize/restore, clear, pause, or resume flows.

Useful tuning variables:

```bash
WARDIAN_E2E_RENDERING_WINDOW_WIDTH=1920
WARDIAN_E2E_RENDERING_WINDOW_HEIGHT=1080
WARDIAN_E2E_RENDERING_RESIZED_WIDTH=980
WARDIAN_E2E_RENDERING_RESIZED_HEIGHT=980
WARDIAN_E2E_RENDERING_WIDE_WIDTH=1920
WARDIAN_E2E_RENDERING_WIDE_HEIGHT=1080
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
WARDIAN_E2E_RENDERING_PI_MODEL=<optional-pi-model>
```

The default `initial`, `settled`, and `wide` visual states use a 1920x1080 desktop window so PR evidence resembles a normal fullscreen desktop. Keep the smaller `resized`, `narrow`, rapid-resize, geometry sweep, and outside-terminal sizes when the test is deliberately proving wrapping, cramped layout, or resize behavior.

The lab sends the configured input text as PTY keystrokes and, by default, submits it with carriage return (`\r`). This is intentional: the real-provider run must create actual conversation history before resize, scrollback, clear, pause, and resume evidence is captured. When `WARDIAN_E2E_RENDERING_INPUT_TEXT` is unset, the default prompt asks the provider to print 50 numbered lines from `WARDIAN_SCROLL_001` through `WARDIAN_SCROLL_050`, and the expected response marker defaults to `WARDIAN_SCROLL_050`. For custom deterministic history checks, make the input text ask for a short marker and set `WARDIAN_E2E_RENDERING_EXPECT_RESPONSE_TEXT` to that marker. Set `WARDIAN_E2E_RENDERING_SUBMIT_INPUT=0` only when intentionally inspecting prompt-editing behavior without a completed provider turn.

For OpenCode rendering runs, the lab defaults to the free remote OpenCode model `opencode/deepseek-v4-flash-free` so the provider does not fall back to local model backends such as LM Studio. Override `WARDIAN_E2E_RENDERING_OPENCODE_MODEL` only when intentionally testing a different OpenCode model.

The lab fails the run for obvious Wardian-side evidence problems before manual screenshot inspection: non-empty screenshot requirements, missing fixed audit text after resize, unchanged columns when a resize state expects a geometry change, screen rectangle mismatch against xterm cell metrics, paused-buffer mismatch, and rendered rows that do not stabilize before the settle timeout. Outside-terminal parity is still captured separately with `scripts/capture-outside-provider-rendering.ps1` when side-by-side native Windows Terminal evidence is needed.
