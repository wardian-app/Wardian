# Agent lifecycle safety

## Decision

Wardian uses distinct lifecycle terms for distinct data effects:

- **Restart Session** restarts the provider while retaining the agent identity,
  habitat, and saved session history.
- **Start Fresh** creates a new provider context while retaining the Wardian
  agent, habitat, and saved history.
- **Delete Agent** permanently removes the Wardian agent, its habitat, and its
  session history. It does not remove project workspace files.

The CLI exposes the safe reclass sequence as `wardian agent update … --class …`
followed by `wardian agent restart …`. Destructive `wardian agent kill` requires
an explicit `--confirm` acknowledgement.

## Rationale

Changing an agent class requires a provider restart to apply updated instructions.
It must not require deleting the agent or its accumulated Wardian state. Explicit
terms and a confirmation flag make the irreversible boundary visible to both
operators and automation.
