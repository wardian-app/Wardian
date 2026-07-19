# Files and Artifact Review Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the approved desktop Files surface and artifact-review workflow without exposing a partial or insecure contribution in New Surface.

**Architecture:** Four dependent plans build the backend-owned file foundation, durable artifact protocol, prompt-scoped review loop, and isolated live renderer in that order. Each slice is independently reviewable, but `CORE_SURFACE_CONTRIBUTIONS` keeps Files reserved until the final activation task proves the complete contract.

**Tech Stack:** Rust, Tauri 2, React 19, TypeScript, Zustand, Dockview, Monaco Editor, PDF.js, Vitest, Playwright browser E2E, and Wardian native E2E.

## Global Constraints

- The approved product contract is `docs/specs/2026-07-16-files-artifact-review-surface.md`; implementation cannot silently narrow it.
- One Workbench surface is labeled **Files**. A mutable file and an artifact thread remain distinct resource identities.
- `AgentConfig.folder` plus `include_directories` are agent-authorized roots. `system_include_directories` are never publication roots.
- Canonicalize paths and resolve symlinks or junctions before containment checks.
- Preview is the default; Draft is explicit. **Apply to file** and **Send to agent** are independent.
- Artifact presentation never steals focus. It opens or reuses a permanent background tab and adds attention.
- Prompt checkpoints are recorded before delivery of user-originated prompts and establish time scope, not authorship.
- Live HTML and active SVG are networkless and receive no Wardian DOM, Tauri, filesystem, clipboard, storage, popup, download, form, or navigation privilege.
- Mobile and remote clients show an unsupported Files state and never mount renderers or request terminal takeover.
- All DTO properties are `snake_case`; all UI colors use Wardian theme variables or themed classes.
- Files remains a reserved contribution until Plan 4, Task 6.

---

## Execution Order

1. [File Surface Foundation](./2026-07-16-files-surface-foundation.md)
   - Establishes canonical resource identity, authorization, stable file revisions, renderer descriptors, desktop renderers, transient preview tabs, and Explorer routing.
   - Produces the `files` surface definition internally but does not expose it in New Surface.
2. [Artifact Lifecycle](./2026-07-16-artifact-lifecycle.md)
   - Adds durable threads and versions, provider-neutral CLI/control requests, background attention, Queue discovery, Quick Open/recent integration, and review retrieval.
   - Depends on Plan 1 file descriptors, authorization, watches, and Workbench resource routing.
3. [Artifact Review and Prompt Changes](./2026-07-16-artifact-review-and-prompt-changes.md)
   - Adds content indexing and prompt checkpoints, Changes mode, durable drafts, conflict handling, annotations, review delivery, and explicit comment-state transitions.
   - Depends on Plan 2 persistence and origin/provenance contracts.
4. [Live Artifact Isolation and Files Activation](./2026-07-16-live-artifact-isolation-and-files-activation.md)
   - Proves a zero-capability live-document host, adds the local dependency broker, hardens retention and renderer lifecycle, completes native E2E and screenshot evidence, and activates Files.
   - Depends on every earlier plan and is the only plan allowed to clear `reserved: true`.

## Cross-Plan Interface Ledger

| Interface | Owner | First consumer | Stability rule |
|---|---|---|---|
| `AuthorizedRootService` | Plan 1 | Plan 1 file open | Plan 4 broker must reuse it rather than duplicate containment logic. |
| `FileContentDescriptorV1` | Plan 1 | Files renderers | Additive fields only during these four plans. |
| `FileResourceEventV1` | Plan 1 | `useFileResource` | One stable revision after debounce; no raw watcher bursts. |
| `ArtifactManifestV1` and blob store | Plan 2 | artifact CLI and Files controller | Immutable version blobs; atomic manifests. |
| `ArtifactPresentedEventV1` | Plan 2 | Workbench/Queue routing | Background focus semantics are fixed. |
| `PromptChangeTracker` | Plan 3 | Changes mode | Explorer is a future consumer of the same service. |
| `DraftRecordV1` and `ArtifactReviewV1` | Plan 3 | Draft and Review UI | Disk apply and agent send remain independent states. |
| `ArtifactCapabilityBroker` | Plan 4 | live HTML/SVG host | Network is denied unconditionally in v1. |

## Pull Request and Issue Sequence

- Every PR links primary issue #392 and the slice-specific related issue when applicable: #393 for contextual file entry points and #395 for Queue review cards.
- Each PR uses the repository template, records exact verification commands, and states that Files is still reserved until the final PR.
- Any frontend PR captures a feature-specific screenshot under `e2e/screenshots/files-surface/<timestamp>/` and embeds a representative HTTPS-hosted image in the PR body.
- Do not merge a slice if its exported interface conflicts with a later plan. Update the later plan in the same PR when an implementation discovery changes an interface.

## Final Release Gate

The final activation commit is allowed only after all of these are true:

- The complete browser suite and targeted Files browser specs pass.
- The workspace Rust suite and targeted core/Tauri tests pass.
- Native E2E proves authorization, watching, presentation/reuse, restart restoration, prompt checkpoint ordering, draft conflict handling, review retrieval, and live-host denial behavior.
- `npm run lint`, `npm run test`, `npm run build`, `npm run test:e2e`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace -- --test-threads=1` pass.
- Desktop Files has a real unsupported state on mobile/remote rather than a hidden crash path.
- The security proof confirms live content cannot reach `window.__TAURI__`, parent DOM, Wardian IPC, arbitrary local files, or any network origin.
- User and developer docs describe Preview, Changes, Draft, review, authorization, unsupported states, and CLI semantics.
- Screenshot evidence is embedded in the final frontend PR.
