# Bounded resource surfaces

Status: implementation complete; PR validation pending

## Problem

Several Wardian commands and views materialize every filesystem entry, Git
change, library object, workflow run, or interaction record before the user
can see a result. A large workspace or long-lived installation can therefore
turn a routine refresh into an unbounded read, allocation, serialization, and
render operation. The failure mode is especially damaging in a multi-agent
workspace because one pathological surface can stall the rest of the desktop.

The contract for these surfaces is now: enumerate only a bounded working set,
make truncation explicit, and keep destructive actions scoped to the items
that were actually returned. A limit is not permission to pretend the result
is complete.

## Scope and limits

| Surface | Boundary | Limit | Overflow behavior |
| --- | --- | ---: | --- |
| Explorer directory listing | `get_directory_tree` | 500 children per directory | response carries `truncated`; Explorer and Garden show a partial-list warning |
| Git status | `git_status` | 1,000 status entries | response carries `files_truncated`; Source Control shows a warning and does not claim a complete count |
| Library index | `get_library_index` | 1,000 filesystem nodes per section; 2,000 deployment records | section carries `truncated`; the Library identifies the partial section |
| Workflow blueprint catalog | `workflow_list_blueprints` | 500 parsed blueprints | response carries `truncated`; the workflow selector identifies the partial catalog |
| Workflow run list | `workflow_list_runs` | 200 newest runs | response carries `truncated`; Observe shows the partial-result notice |
| Inbox notification projection | `list_inbox_notifications` | 200 newest notifications | response carries `truncated`; Inbox identifies the partial projection |
| Topology activity | `get_pair_activity` | 5,000 recent records and 1,000 pair rows | result carries `truncated`; the graph treats the activity set as a quiet recent projection |

The constants live next to the owning boundary so a caller cannot request an
unbounded page through the public command. Limits count returned domain
objects, not UI rows. A folder header, status entry, run summary, or pair row
is one object for the purpose of its boundary.

## API rules

1. A bounded result is represented by a named response object when the caller
   needs to distinguish an empty complete result from an empty partial result.
2. `truncated: true` means that at least one eligible object was omitted. The
   caller must not infer an exact total from the returned length.
3. Ordering is deterministic. Filesystem and Git results retain their current
   directory/status ordering; workflow runs are newest first; topology
   activity is newest first for the retained records/pairs.
4. Mutations operate only on returned objects. Bulk controls must not claim to
   stage, discard, or otherwise affect omitted objects.
5. A limit applies before serialization and rendering. UI-only slicing is a
   fallback guard, not the primary resource boundary.

## Reviewed surfaces

The following already have a bounded or intentionally lazy boundary and are
kept in regression coverage rather than changed in this pass:

- Workflow monitor history uses paged/virtualized event rows.
- Chat transcript previews and file-resource reads have explicit byte/row
  limits.
- Terminal output is retained in bounded buffers.
- The Inbox queue uses lazy visible-item rendering; its persisted notification projection is capped at 200 newest records in this pass.
- The workflow launch agent picker is searchable and limits visible choices.
- Agents Overview limits resident terminal renderers even though the roster
  itself remains an intentional all-agent view.

The following are deferred because they are administrative exports or have a
separate pagination contract: full Git history/detail, explicit diff hunk
loads, and operator-wide agent roster management. They must not reuse the
bounded UI response types without an explicit review.

## Acceptance criteria

- A large directory, Git worktree, library section, workflow log, or
  interaction database cannot force the corresponding public command to
  return an unbounded collection.
- Every truncated response is visible to the consuming surface, carried forward
  to the next API boundary, or explicitly defined as a bounded projection by
  that surface; no partial result is presented as complete.
- Existing small-result behavior and ordering remain unchanged.
- Unit tests exercise the limit and the `truncated` transition for every
  changed response, plus a UI test for each user-visible overflow notice.
- The generated documentation and the implementation agree on the constants
  and response semantics.

## Bounded expansion

The initial cap is a page size, not a permanent visibility cutoff. Every
surface that reports `truncated` exposes a continuation affordance when a
continuation token is available. Continuations are offset/cursor based and
use the same fixed page size as the initial request (500, 200, 1,000, or
5,000, as appropriate); a request never grows with the number of pages the
operator has already loaded. The UI may append pages for inspection, but no
single response or expansion action may return the full collection.

The contract applies to indirect consumers too: Garden folder/workflow views,
the Graph activity view, and other projections preserve the bounded-result
semantics. The Graph deliberately consumes only its initial recent projection;
it does not surface an overflow notice or continuation control. The backend
command retains its offset contract for callers that need historical expansion.
When a collection is refreshed, a fresh initial request resets the continuation
sequence so pages cannot silently splice together different snapshots.

## Review checklist

- [x] Source Control status files
- [x] Workspace Explorer directory children
- [x] Library index recursion and section rows
- [x] Workflow run summaries
- [x] Topology interaction history and pair activity
- [x] Workflow blueprint catalog and Inbox notification projection
- [x] Existing bounded transcript, terminal, monitor, and queue surfaces
- [x] Native/real-provider acceptance is not required; these are local storage,
  command, and rendering limits.

## Graph activity presentation

The Graph is a visual topology surface, not a communication-history browser.
It renders the initial bounded recent activity projection without showing a
truncation banner or a page-expansion control. This keeps routine long-lived
workspaces free of a persistent warning while the backend cap remains in place.

## Verification

- Rust unit coverage exercises directory, Git, library, workflow, topology,
  and workflow-catalog limits.
- Frontend tests cover every visible truncation notice.
- The workbench browser E2E suite passes, including a real Playwright capture
  of the workflow-catalog notice.
- Native/provider acceptance is intentionally out of scope: these boundaries
  protect local enumeration and rendering before provider execution begins.
