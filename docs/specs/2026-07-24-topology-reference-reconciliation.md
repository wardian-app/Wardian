# Spec: Topology Reference Reconciliation

- **Status**: Implemented
- **Issue**: [#726](https://github.com/wardian-app/Wardian/issues/726)
- **Related**: `docs/specs/2026-07-02-communication-topology.md`

## Decision

`topology.json` references agents by UUID, while the roster can change independently through legacy state, direct file edits, interrupted lifecycle operations, or older builds. A graph surface must therefore never present a connection whose endpoint is absent from the current roster.

Wardian reconciles topology against the current roster whenever the desktop Graph view or a `wardian graph` command loads it. The shared core helper removes records with an unknown endpoint and atomically saves only when reconciliation changed the file.

## Scope

- Remove stale manual edges, ignored ghost pairs, and team-seed suppression pairs.
- Preserve all records whose two endpoints are still in the roster.
- Reuse the same helper for normal desktop deletion cleanup.
- Keep generic resolver and messaging paths non-mutating; they continue to filter unknown UUIDs defensively.

## Verification

- Core coverage seeds a persisted topology with valid and stale records, verifies the repaired file, and verifies a second pass is a no-op.
- CLI integration coverage verifies `wardian graph show` both omits dangling records and persists the repair.
