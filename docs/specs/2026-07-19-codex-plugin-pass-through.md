# Codex Plugin Pass-Through

## Context

Wardian gives every Codex agent a separate `CODEX_HOME` so sessions, history,
SQLite state, memories, project trust, and local overrides stay isolated. The
provider previously added a second, class-specific plugin policy on top of that
home. It installed selected plugins, wrote an agent-local policy-status cache,
and globally disabled plugin and app features for other launches.

That layer made the observed plugin state inconsistent: a plugin could be
enabled in the agent home while the cache reported it absent, and a launch flag
could suppress it regardless of the home configuration.

## Decision

Keep the existing base-plus-agent-overlay configuration projection, but treat
the resulting per-agent Codex home as the plugin source of truth:

- Wardian does not select, install, enable, disable, or remove Codex plugins.
- Wardian does not append global `--disable plugins` or `--disable apps`
  switches to normal Codex launches.
- The existing approval settings remain unchanged; this decision does not add a
  bypass for Codex or plugin confirmation controls.
- The agent doctor reports the effective home, resolved launch flags, and the
  installed/enabled plugin list obtained from that home. Inspection is
  read-only and uses the provider's executable resolution rather than a
  separately-invoked command name.

## Operational Constraint

Codex fixes a session's tool list when the session starts. After changing
plugin or configuration state, start a fresh Codex session rather than
resuming an earlier thread to see the changed tools.

## Verification

The provider launch-argument tests assert that Coder, Electrical Engineer, and
Mechanical Engineer launches receive no plugin-specific global disable flags.
The home-inspection parser test covers both enabled and disabled installed
plugins. A live inspection of the SomaBeats Electrical Engineer home returned
`computer-use@openai-bundled` as installed and enabled; its existing resumed
thread still requires a fresh session to acquire that tool list.
