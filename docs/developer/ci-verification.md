# Local CI verification

Run the project-owned core CI checks with:

```sh
npm run verify:ci
```

To rerun one category after a failure:

```sh
npm run verify:ci -- --only backend
```

The runner reads the marked `run:` steps from `.github/workflows/ci.yml`, so
the command list and flags stay coupled to CI. It fails fast and echoes the
literal failing command. Use `npm run verify:ci -- --list` to inspect the
resolved sequence without executing it.

`--only` accepts exactly one of `frontend`, `backend`, or `docs`. Missing,
empty, unknown, and repeated category options fail before any check starts.
Each workflow marker must immediately precede an equally indented, literal
single-line `run:` command. YAML block scalars are not supported.

Backend verification lints and tests all workspace targets, including tests
and examples. Documentation tests run separately because `--all-targets` does
not include them. The command-contract tests in `src/verify-ci.test.ts` pin
this coverage and exercise invalid arguments and workflow declarations.

For provider fixtures, shared environment locks, and deliberate contention,
use [Test Reliability](./test-reliability.md). That guide maps each pattern to
its executable check and states the limits of the evidence.

The local sequence covers the frontend, backend, and documentation quality
steps. PR screenshot/code-claim checks, dependency audits, coverage uploads,
and browser/native suites remain CI- or environment-specific.

The hosted `Backend (macOS - Codex Home ACL)` job runs the focused Codex home
platform tests on `macos-latest`. Its native fixtures check that a private
directory without an extended ACL is accepted and one with an ACL entry is
rejected. This is filesystem validation, not a Codex spawn or provider test;
that runtime acceptance still requires a separate Mac run.

A local pass does not complete PR delivery. Follow [Pull Request
Delivery](./pull-requests.md) to monitor hosted checks on the latest published
commit, resolve failures, and verify that all applicable checks have finished
successfully before declaring the task complete.
