# Communication Topology

Use `wardian graph` to inspect and change the communication topology that
shapes default agent visibility and messaging scope. It is the same topology
shown in Wardian's Graph view.

## Inspect The Graph

```bash
wardian graph show
wardian graph neighbors
wardian graph neighbors coder-a1
wardian graph activity
```

`show` returns agents, manual edges, unmapped pairs, and ignored pairs.
`neighbors` resolves one agent's visible peers; without a target it resolves
the current agent and therefore requires a managed session. `activity` reports
each pair's last message, open asks, and whether it is an unmapped suggestion.

The topology explains why `agent list`, broadcasts, class sends, and bare-name
resolution are neighbor-scoped by default. See [messaging](messaging.md) before
using a cross-community send.

## Change Communication Boundaries

```bash
wardian graph link architect-a1
wardian graph unlink architect-a1
wardian graph ignore fork-coder
wardian graph unignore fork-coder
```

Inside a Wardian-managed terminal, each edit must involve the calling agent;
for example, `link <other>` connects the caller to that peer. Outside a session
an operator may pass both endpoints, such as `wardian graph link <agent-a>
<agent-b>`. Use `link` to formalize an unmapped (ghost) pair and `ignore` to
durably dismiss it. The commands are idempotent: a repeated edit reports
`"changed": false` and exits successfully.

Edits are written to the inspectable topology state and a running app refreshes
the Graph view live. Prefer these commands over hand-editing topology state so
identity and self-service rules are enforced.
