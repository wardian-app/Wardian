# Spec 042: Agent ID Display

- **Status:** Implemented
- **Date:** 2026-05-18
- **Decider:** User

## Context and Problem Statement

Wardian stores a stable Wardian agent ID in `session_id` and may also store a provider-specific resume/thread ID in `resume_session`. The Configure Agent panel previously displayed `resume_session || session_id` in a field labeled "Session ID", while the Copy button and Wardian CLI exposed `session_id`.

That made the UI appear inconsistent and leaked provider runtime details into normal agent identity. The mismatch was especially confusing for providers with non-UUID session formats, such as OpenCode `ses_...` IDs.

## Proposed Decision

User-facing identity surfaces should expose the Wardian agent ID only. Provider resume IDs remain internal runtime metadata used by spawn, resume, telemetry, and log lookup flows.

The Configure Agent panel should:

- Label the visible identifier as `Agent ID`.
- Display `config.session_id`, regardless of `resume_session`.
- Copy `config.session_id` from the adjacent Copy button.
- Avoid showing `resume_session` in the primary Configure Agent UI.

## Consequences

- **Positive:** Wardian CLI, copy behavior, and the Configure Agent panel present the same stable agent identifier.
- **Positive:** Other agents and users do not need to understand provider-specific resume ID formats.
- **Positive:** Providers can keep independent resume semantics without changing Wardian's public identity model.
- **Negative:** Debugging provider resume state requires logs, persisted state inspection, or future developer-only diagnostics rather than the main configuration panel.
