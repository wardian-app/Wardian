# Provider Clear Session Materialization

## Status

Implemented.

## Problem

Clear must replace an agent's provider conversation without leaving the next
interactive launch pointed at a missing provider session. Prompt-based
bootstraps created unnecessary model turns and could surface provider-specific
interruption state.

## Decision

- Codex Clear writes a minimal, provider-recognized session rollout in the
  agent's projected `CODEX_HOME`, using a newly generated provider UUID. The
  interactive terminal resumes that exact local rollout.
- Antigravity Clear starts a fresh interactive session without a synthetic
  prompt. Wardian records the provider's real conversation ID only after
  Antigravity updates its workspace conversation mapping in response to the
  user's first prompt.

## Consequences

Clear no longer sends the visible or hidden "Introduce yourself" prompt for
Codex or Antigravity. Codex has a resumable session immediately. Antigravity
has no resumable provider ID until its first genuine user interaction; until
then, a restart deliberately starts another fresh Antigravity conversation.
