# Developer Setup

One-time setup steps beyond `npm install` and Rust toolchain installation.

## Required Tools

### Rust Toolchain

Install [rustup](https://rustup.rs/), then run these commands inside the repository
in Bash or PowerShell:

```bash
rustup show
rustc --version
cargo --version
```

Rustup installs and selects the exact compiler in the root `rust-toolchain.toml`,
including Clippy and rustfmt. CI uses the same version. The
[Rust cache launcher](./rust-build-cache.md) also respects this pin; no global
default-toolchain change is needed. Release jobs install their matrix-specific
cross-compilation targets separately.

To bump Rust, open a dedicated PR updating `channel` in `rust-toolchain.toml`
and every `dtolnay/rust-toolchain@<version>` reference in `.github/workflows/`.
Choose an exact stable release, run the commands above to confirm the compiler,
and run `npm run verify:ci`. Fix any newly enabled lints in that PR without
suppressing them. Confirm the logged `rustc --version` in every Rust CI job
matches the pin before merging. The toolchain regression test rejects drift
between the manifest and workflow installers.

### cargo-llvm-cov (Rust coverage)

Required for `npm run test:coverage:rust`. Install once:

```bash
cargo install cargo-llvm-cov --locked
```

This generates `coverage/rust-lcov.info` which is uploaded to Codecov in CI. The `--locked` flag ensures the version matches CI.

### Agent Provider CLIs

Install and authenticate at least one supported provider CLI before running real agents. Native and real-provider E2E work may require the specific provider under test.

| Provider | Project | Common CLI Command |
|---|---|---|
| Antigravity | [Antigravity CLI overview](https://www.antigravity.google/docs/cli-overview) | `agy` |
| Claude Code | [anthropics/claude-code](https://github.com/anthropics/claude-code) | `claude` |
| Codex | [openai/codex](https://github.com/openai/codex) | `codex` |
| OpenCode | [anomalyco/opencode](https://github.com/anomalyco/opencode) | `opencode` |
| Pi | [Pi coding agent documentation](https://pi.dev/docs/latest/) | `pi` |
| Gemini CLI *(unmaintained)* | [google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli) | `gemini` |

## Optional Tools

### Playwright browsers

Required for browser E2E tests (`npm run test:e2e`) and local feature-specific screenshot capture:

```bash
npx playwright install --with-deps chromium
```

### Native E2E harness

Required for `npm run test:e2e:native`. See [native-e2e.md](./native-e2e.md) for full setup:

```bash
npm run setup:e2e:native
```

## Local Builds

Run the desktop app in development mode:

```bash
npm run dev
```

Debug desktop builds do not use the production Wardian home by default. If `WARDIAN_HOME` is unset, or if a managed agent shell inherited the default production home, the dev app rewrites its process environment to `target/debug/.wardian` before opening the database, control endpoint, remote gateway, or restored agents.

When testing the CLI against a dev app, set the same explicit `WARDIAN_HOME` in both terminals:

macOS/Linux shell:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
npm run dev
```

Then run CLI commands from another terminal with that same home:

```bash
export WARDIAN_HOME="$PWD/.tmp/wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
npm run dev
```

Then run CLI commands from another terminal with that same home:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all
```

With the app running, CLI output comes from the live desktop endpoint. Request `status_source` explicitly when you need to verify that path:

```bash
cargo run -p wardian-cli -- agent list --scope all --fields name,status,status_source
```

If the app is stopped, the same command falls back to `state.db` and reports `"status_source": "persisted"`.

Build the standalone CLI:

```bash
cargo build -p wardian-cli
```

Stage the release CLI into the Tauri resource directory used by bundling:

```bash
npm run stage-cli
```

Build a local production desktop bundle for the current platform:

```bash
npm run tauri build
```

This creates an installable local bundle that uses your normal Wardian state directory (`~/.wardian` by default). Local bundles do not create updater artifacts and do not require `TAURI_SIGNING_PRIVATE_KEY`.

This is the complete local desktop-build command. Run it from the repository root in a normal terminal session; it does not require a running Wardian app, agent, or special environment. The release profile does not reuse Rust incremental artifacts, avoiding stale-cache compiler crashes after toolchain changes.

PowerShell:

```powershell
npm run tauri build
```

Official release builds opt into signed updater artifacts with the release-only Tauri config overlay. Do not use the release overlay for ordinary local builds.

## Environment Variables

| Variable | Purpose | Default |
|---|---|---|
| `WARDIAN_HOME` | Redirect all state to an isolated directory | `~/.wardian` |
| `WARDIAN_DEBUG_ALLOW_PRODUCTION_HOME` | Allow a debug desktop build to use the default production home when set to `1` | unset |
| `WARDIAN_MOCK_SCENARIO` | Mock provider scenario for native E2E tests | `basic` |
| `WARDIAN_MOCK_DELAY_MS` | Delay between mock provider events (ms) | `100` |
| `WARDIAN_E2E_REAL_ANTIGRAVITY` | Enable real Antigravity provider in native E2E | unset |
| `WARDIAN_E2E_REAL_OPENCODE` | Enable real OpenCode provider in native E2E | unset |

## Running Unit Tests

```bash
npm run test                     # frontend
cd src-tauri && cargo test       # backend
```

Rust tests that read or write environment variables must run with
`--test-threads=1`; the process environment is shared, so parallel tests race
each other and fail nondeterministically.

## Running Coverage Reports

### Frontend

```bash
npm run test:coverage
# Output: coverage/lcov.info
```

### Backend

```bash
npm run test:coverage:rust
# Output: coverage/rust-lcov.info
```

Coverage folders are gitignored. CI uploads them to Codecov automatically on PRs.
