# Public Positioning and Habitat Language

- **Status:** Accepted
- **Date:** 2026-07-02
- **Issue:** [#513](https://github.com/wardian-app/Wardian/issues/513)
- **Decision owner:** Maintainers

## Context and Problem Statement

Wardian's public copy has often described the product as a "command center" for
agent teams. That phrase is understandable, but it is generic and does not
match the product direction captured in the HabitatLayout epic.

Epic #513 names the more durable product model: Wardian is a Habitat made of
Sites, Cohorts, perspectives, surfaces, Garden, Graph, and explicit lifecycle
boundaries. Public-facing explanations should introduce that direction without
claiming every HabitatLayout feature already exists.

## Decision

Use **local-first desktop habitat** as the default public one-line framing for
Wardian. Public copy should emphasize:

- bring-your-own agent tools that run locally;
- durable identity, terminals, provider runtime state, Queue evidence, and
  workflows;
- reusable local artifacts such as prompts, classes, skills, and workflows;
- the HabitatLayout direction toward Sites, Cohorts, movable surfaces, Garden
  as a spatial operating view, and Graph as communication topology.

Avoid "command center" as the leading public product metaphor. It may remain in
historical research or old implementation specs when describing prior thinking
or third-party reference categories, but new user-facing copy should prefer
Habitat, Site, Cohort, surface, roster, Queue, Garden, Graph, and local-first
runtime language.

## Consequences

- README, app metadata, website metadata, and docs summaries should no longer
  lead with "command center."
- Current guides should describe the UI that exists today while noting the
  HabitatLayout direction where useful.
- Future docs should distinguish agent/project workspaces from UI Sites so the
  term "workspace" does not blur filesystem runtime state with saved UI
  operating places.
