# Continuous Integration & Quality Gates

- **Status:** Implemented
- **Date:** 2026-03-25
- **Decider:** Architect

## Context and Problem Statement

As Wardian moves to a multi-developer, PR-based workflow, we need automated checks to ensure that new code doesn't break existing functionality, introduce linting errors, or fail to compile across different operating systems.

## Proposed Decision

We will implement **GitHub Actions** as our primary CI engine.

### 1. The CI Workflow (`.github/workflows/ci.yml`)

This workflow triggers on every Pull Request to `main` and on every push to `main`.

#### Jobs:

- **`security-audit`**: Runs `cargo audit` and `npm audit` to detect known vulnerabilities in the supply chain.
- **`frontend-quality`**: Runs `npm run lint` and `npm run test` on `ubuntu-latest`.
- **`backend-windows`**: Runs `cargo clippy` and `cargo test` on `windows-latest` to verify Win32-specific integrations.
- **`backend-linux-check`**: Runs `cargo check` on `ubuntu-latest` to ensure cross-platform compilation integrity.

### 2. Pull Request Standardization

A `.github/PULL_REQUEST_TEMPLATE.md` is provided to ensure all contributors provide context, issue references, and testing verification for their changes.

### 3. Branch Protection (Recommendation)

We recommend enabling "Require status checks to pass before merging" in GitHub settings once this PR is merged.

## Consequences

- **Positive**: Guaranteed baseline quality for every merge.
- **Positive**: Professional, standardized contribution workflow.
- **Negative**: Increased build times for PRs (mitigated by Rust caching).
