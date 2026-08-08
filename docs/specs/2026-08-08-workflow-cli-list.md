# Workflow CLI Discovery

## Problem

The workflow CLI could validate, execute, and inspect runs, but it could not
enumerate the blueprint files available in the Library. Users had to filter
`wardian library list` manually or infer availability from prior runs. That is
especially misleading when a workflow's declared id differs from its filename.

## Decision

Add `wardian workflow list` as a read-only Library discovery command.

- Default output is the versioned JSON envelope `{ "schema": 1, "workflows": [...] }`.
- Each row includes the parsed `blueprint_id`, display `name`, Library
  `entry_ref`, absolute `workflow_path`, and nullable `error`.
- Blueprint ids and display names come from parsing each blueprint's frontmatter;
  filenames are never used as ids.
- A parse failure is isolated to its row so one malformed Library file does not
  hide other discoverable workflows. The fallback name remains the Library
  filename stem and `blueprint_id` is null.
- `--pretty` emits one human-readable row per Library entry, consistent with
  the CLI's other output modifiers.

## Scope

This command reads the existing Library index and workflow parser. It does not
change workflow execution, scheduling, or frontend behavior.
