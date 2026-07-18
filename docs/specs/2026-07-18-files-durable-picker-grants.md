# Files Durable Picker Grants

- **Status:** Approved
- **Date:** 2026-07-18
- **Primary issue:** [#392](https://github.com/wardian-app/Wardian/issues/392)
- **Extends:** `docs/specs/2026-07-17-files-subscription-authorization-provenance.md`

## Context

An ordinary file selected through the native picker can be outside every agent
primary or additional directory. Wardian previously retained that exact-file
grant only in memory, while the Workbench correctly persisted only the file
identity. Relaunch therefore restored the tab but discarded the authority that
made the tab usable.

Windows canonical paths also reached persisted resource keys with the
extended-length `\\?\` prefix converted to `//?/`. That spelling is useful to
filesystem APIs but is not an appropriate visible or durable Workbench
identity.

## Decision

Native picker selections and successful Save As targets are durable exact-file
grants. The Rust backend stores a bounded least-recently-used list of canonical
paths in `settings/file-grants.json`. Capability identifiers, retained handles,
revision tokens, and file content are never serialized.

On trusted Workbench restore, Wardian first checks current agent primary and
additional directories. If none authorize the request, the backend canonicalizes
the requested path and compares it with the durable exact-path registry. An
exact match reauthorizes the ordinary file and mints a new live capability and
retained handle. A parent, sibling, alias retarget, or edited Workbench resource
key gains no authority.

Runtime shutdown still closes every watcher, subscription, ticket, handle, and
live capability. It does not erase the backend-owned durable path decision.
The registry remains capped at the same 128 entries as live picker grants, and
least-recently-used entries are evicted as new files are selected or restored.

Windows resource identity strips extended drive and UNC prefixes before the
key enters Workbench state. Existing `file://?/C:/...` keys are migrated by the
frontend decoder and rewritten to the ordinary backend identity after a
successful reopen. The header keeps the complete normalized path in the DOM
and uses ordinary CSS overflow instead of inserting a synthetic `…` segment.

## Required Invariants

1. Relaunch does not revoke a file explicitly selected through the native
   picker or created through Save As.
2. Durable records contain canonical paths only; capability and revision tokens
   remain runtime-private.
3. A durable grant authorizes one exact canonical file and never its parent or
   siblings.
4. Workbench layout state alone never grants filesystem access.
5. Every restored use creates and validates a fresh retained handle before any
   descriptor, text, or renderer read.
6. Windows resource keys and visible paths never expose `\\?\` or `//?/`.

## Verification

Rust regression coverage creates one runtime, records a picker grant, closes
all runtime resources, creates a second runtime over the same grant store, and
proves that the selected file reopens while a sibling remains unauthorized.
The test also proves that the persisted JSON does not contain the live
capability identifier. Frontend tests cover migration of legacy extended-path
resource keys and full-path header rendering.
