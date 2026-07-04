# Team Edge Deletion Suppression

- **Status:** Accepted
- **Date:** 2026-07-03
- **Issue:** [#633](https://github.com/wardian-app/Wardian/issues/633)
- **Related:** [#610](https://github.com/wardian-app/Wardian/issues/610)
- **Decision owner:** Maintainers

## Context

Communication topology stores real, editable edges in
`<WARDIAN_HOME>/topology.json`. Teams seed clique edges into that topology so a
newly created team has useful neighbor defaults, but seeded edges become user
owned once written.

The original schema could not distinguish "this team pair has never been
seeded" from "this team pair was seeded and the user intentionally deleted it."
That made later team-seed passes capable of recreating a deleted edge while the
team membership still existed.

## Decision

Topology schema version 3 adds `suppressed_seed_pairs`, a canonical list of
agent pairs that team membership must not recreate. Removing an edge whose pair
is still present in any team deletes the edge and adds the pair to
`suppressed_seed_pairs`. Adding the edge manually clears that suppression,
because the user has reconnected the pair.

`ignored_pairs` remains separate. It dismisses ghost communication suggestions
and workspace-fallback visibility; it must not be reused for team-seed
suppression because disconnecting a team-born edge should not hide future
activity or fallback signals for that pair.

## Consequences

- Team-created edges remain fully editable and stay deleted across app restarts
  and future team-seed passes.
- Existing version 2 topology files are not reseeded merely because schema
  version 3 exists. The one-time team seed still applies only to pre-version-2
  topology files or a missing topology file.
- `topology.json` remains inspectable Markdown-as-truth-adjacent state: the
  durable user intent is visible as `suppressed_seed_pairs`.

## Testing

Core topology tests cover:

- team clique seeding skips suppressed pairs;
- removing a team-backed edge records suppression;
- manually adding the edge clears suppression.
