# AGENTS-only provider instructions

- **Status:** Implemented
- **Date:** 2026-09-18
- **Issue:** #1377

## Context

Wardian uses `AGENTS.md` as the canonical common, class, agent, and generated
habitat instruction source. It previously generated sibling `CLAUDE.md` and
`GEMINI.md` compatibility files. Claude now reads `AGENTS.md` natively, while
Wardian's Gemini integration is unmaintained, so those copies no longer have an
active supported consumer.

## Decision

Wardian generates only `AGENTS.md`. New class and habitat roots do not receive
provider-specific instruction files, and Claude reports `AGENTS.md` as its
instruction filename. The repository root no longer carries compatibility
stubs.

Home initialization migrates existing Wardian-managed common, class, agent,
and habitat roots. It removes an exact legacy `@AGENTS.md` stub, or an unchanged
Claude projection whose body still matches its Wardian SHA-256 ownership
marker. It preserves customized content, symbolic links, junctions, hardlinks,
unknown formats, workspace files, and user-selected include directories.

The migration is idempotent. A partial run can be retried because absent files
are already in the target state and preserved files remain outside Wardian's
ownership proof. No rollback writer recreates the retired files; recovery for a
mistakenly customized file is preservation, not regeneration.

## Consequences

- `AGENTS.md` remains the single inspectable source of truth.
- Supported Claude launches no longer depend on copied snapshots or
  `@AGENTS.md` imports.
- Existing user-authored provider files remain available as explicit overrides,
  but Wardian does not create or refresh them.
- Legacy Gemini execution may require user-managed provider configuration and
  remains unsupported.

## Verification boundary

Core filesystem tests cover generated-file recognition, full managed-root
retirement, idempotence through absence, and preservation of custom files and
hardlinks. Class, CLI, habitat, headless, and provider tests assert the target
`AGENTS.md`-only state. These checks establish Wardian's filesystem contract;
they do not independently certify a particular external Claude release.
