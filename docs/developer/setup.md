# Developer Setup

One-time setup steps beyond `npm install` and Rust toolchain installation.

## Required Tools

### Rust Toolchain

Install via [rustup](https://rustup.rs/). The stable channel is sufficient:

```bash
rustup toolchain install stable
```

### cargo-llvm-cov (Rust coverage)

Required for `npm run test:coverage:rust`. Install once:

```bash
cargo install cargo-llvm-cov --locked
```

This generates `coverage/rust-lcov.info` which is uploaded to Codecov in CI. The `--locked` flag ensures the version matches CI.

## Optional Tools

### Playwright browsers

Required for browser E2E tests (`npm run test:e2e`, `npm run screenshots`):

```bash
npx playwright install --with-deps chromium
```

### Native E2E harness

Required for `npm run test:e2e:native`. See [native-e2e.md](./native-e2e.md) for full setup:

```bash
npm run setup:e2e:native
```

## Environment Variables

| Variable | Purpose | Default |
|---|---|---|
| `WARDIAN_HOME` | Redirect all state to an isolated directory | `~/.wardian` |
| `WARDIAN_MOCK_SCENARIO` | Mock provider scenario for native E2E tests | `basic` |
| `WARDIAN_MOCK_DELAY_MS` | Delay between mock provider events (ms) | `100` |
| `WARDIAN_E2E_REAL_OPENCODE` | Enable real OpenCode provider in native E2E | unset |

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
