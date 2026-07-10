# Library CLI Hardening

- **Status:** Approved
- **Date:** 2026-07-09

## Context

The first Library CLI release gives agents direct access to prompts, skills,
classes, workflow blueprint files, metadata, and skill deployments. Live CLI
exercise found several cases where accepted commands produce state that the
Library cannot subsequently index, where destructive commands do not enforce
their advertised preconditions, and where standalone CLI class state differs
from desktop-initialized state.

This pass fixes those concrete correctness and agent-experience gaps. It does
not add cross-process locking. That broader concurrency problem needs a
separate design covering all Wardian writers rather than Library-only locks.

## Decisions

### 1. Enforce Entry Shape in the Shared Core

Library mutation APIs enforce the same shapes consumed by the index:

- prompts and workflow blueprints must use a `.md` extension;
- a skill directory containing `SKILL.md` cannot contain another skill;
- a folder containing descendant skills cannot be converted into a skill;
- skill moves must satisfy the same destination rules as skill creation.

These checks live in `wardian-core::library`, so CLI and desktop callers cannot
create state that the shared index hides. Existing malformed entries remain
deletable so users and agents can repair old state.

Workflow validation, parsing, normalization, execution, scheduling, and run
inspection remain exclusively under `wardian workflow`. The Library check is
only the file-shape invariant required by the Library index.

### 2. Verify Orphans Before Deletion

`wardian library orphan delete` removes a deployment only when the current
deployment scan reports the exact `(target_type, target_id, skill_name)` tuple
as orphaned. A healthy deployment or a stale caller request returns
`not_found` and leaves the filesystem unchanged.

The verification belongs in the shared core helper so every caller receives
the same destructive-operation guarantee.

### 3. Complete Set-Based Deployment Semantics

Deployment target input is normalized into a unique, stable-order list before
reconciliation. Duplicate targets therefore produce one durable operation and
one result entry.

The command accepts exactly one of:

```text
wardian library deploy <skills/ref> --targets <non-empty-target-list>
wardian library deploy <skills/ref> --clear
```

`--clear` reconciles the skill to an empty desired target set. It is explicit
so an unset or empty shell variable passed to `--targets` cannot undeploy a
skill accidentally. No separate add/remove aliases are introduced.

### 4. Make Flat Output Actually Flat

`wardian library list <section> --flat` returns:

```json
{
  "schema": 1,
  "section": "skills",
  "stubbed": false,
  "entries": []
}
```

It does not also include the tree. `wardian library list --flat` returns one
combined `entries` array across all sections. Every row includes `section` in
addition to the existing entry fields. Unscoped flat output omits deployments
and orphans; their dedicated commands remain the efficient agent interfaces.

Tree-shaped output without `--flat` remains backward compatible.

### 5. Initialize and Clean Up Classes Consistently

A reusable core initializer ensures `classes.json`, every class directory,
`AGENTS.md`, and provider instruction stubs exist. Standalone Library CLI
operations invoke it before listing or addressing classes and before class
deployment target validation. Existing files are preserved.

The desktop class initializer reuses the same core behavior after its legacy
`custom_classes.json` migration. Desktop-only provider discovery links remain
in the Tauri layer.

Deleting a custom class removes its `classes/<Name>` Library metadata so a
later class with the same name does not inherit stale tags or star state.

### 6. Use Atomic JSON Replacement Where Touched

Writes to `classes.json` and `library/library.json` serialize to a temporary
file in the destination directory, flush it, and atomically replace the target
using the existing cross-platform replacement behavior. This prevents a crash
from leaving truncated JSON.

Atomic replacement does not prevent two simultaneous load-modify-save writers
from overwriting one another. Cross-process locking and merge semantics are
explicitly deferred.

### 7. Improve Agent-Facing Help

Library commands and non-obvious arguments receive concise Clap documentation.
Help text must explain section-qualified refs, repeated `--set` tags, complete
desired-set deployment semantics, and the workflow boundary where relevant.

Field projection and a CLI-wide compact JSON policy are deferred because they
should be consistent across Wardian namespaces. Correct flat output addresses
the immediate Library payload problem without creating a Library-only output
framework.

## Error Contracts

Existing JSON error envelopes and exit-code behavior remain in place.

- invalid markdown entry shapes return `invalid_ref`;
- invalid nested skill layouts return `invalid_ref`;
- deleting a deployment that is not currently orphaned returns `not_found`;
- empty `--targets` remains invalid;
- supplying both `--targets` and `--clear` is rejected by Clap.

## Testing

Core unit tests cover entry-shape validation, skill ancestor/descendant
conflicts, verified orphan deletion, class initialization, class metadata
cleanup, target deduplication, and atomic JSON replacement.

CLI integration tests reproduce the live failures and prove:

- extensionless prompts/workflows are rejected before files are written;
- nested or folder-promoting skills are rejected without hiding entries;
- healthy deployments survive `orphan delete`;
- `--clear` removes the final deployment;
- duplicate targets report one add and one durable target;
- scoped and unscoped flat output contain entries without trees;
- fresh-home default classes support list/show/read/write/deploy;
- deleted and recreated classes do not inherit metadata;
- workflow entries still hand off through `workflow_path`.

No frontend behavior changes, so browser E2E and screenshot evidence are not
required. Final verification follows the repository pre-commit checklist.

## Consequences

- Agents can rely on successful mutations remaining discoverable through the
  same CLI.
- Destructive orphan cleanup becomes state-checked and retry-safe.
- Deployment reconciliation can represent every desired set, including empty.
- Standalone CLI behavior no longer depends on the desktop app having seeded
  class files first.
- Multi-agent lost-update prevention remains unresolved until a coordinated
  cross-process persistence design is implemented.
