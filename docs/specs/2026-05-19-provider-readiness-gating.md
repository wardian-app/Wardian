# Provider Readiness Gating

- **Status:** Proposed
- **Date:** 2026-05-19

## Context and Problem Statement

Wardian currently presents supported provider engines as if every user has every
provider CLI installed and visible to the app process. When a user selects a
provider that is not installed, not on `PATH`, or not visible from the desktop
app environment, the failure happens late during spawn and the error does not
clearly explain that Wardian can only launch locally installed provider CLIs.

The documentation already explains provider readiness, but the app should make
the constraint visible before the user tries to spawn or clone an agent. The
app also needs a backend guard because agents can still be spawned through the
CLI, workflows, restores, clones, or races where a provider disappears after
the UI loads.

## Proposed Decision

Add provider readiness as a backend-owned capability surface. Wardian will
detect whether each supported provider executable can be resolved from the same
environment and provider-specific fallback logic used for launch. The readiness
check is intentionally limited to executable existence; it will not run
`--version`, authenticate, probe billing, or test workspace trust.

Readiness should be exposed through a Tauri command returning all user-facing
providers:

```json
[
  {
    "provider": "claude",
    "display_name": "Claude",
    "available": true,
    "executable": "<resolved-provider-executable>",
    "reason": null
  },
  {
    "provider": "gemini",
    "display_name": "Gemini",
    "available": false,
    "executable": null,
    "reason": "The gemini command was not found in the Wardian app environment."
  }
]
```

`mock` remains test/internal unless a test path explicitly asks for it. It
should not appear as a normal user-facing provider option.

The spawn, custom clone, and existing-agent configuration UI will list all
supported providers. Unavailable providers are disabled and labeled with setup
state, for example `Gemini - not installed`. The form shows a compact
explanation near the selector: `Only provider CLIs found on this machine are
selectable.` A setup or troubleshooting action should point to the existing
provider readiness guide.

Add a default provider setting:

- Values: `auto`, `claude`, `codex`, `gemini`, `antigravity`, `opencode`
- Default: `auto`
- Auto order: prefer `claude`, then the next available user-facing provider in
  the supported-provider order
- Explicit default: use the selected provider when it is available
- Unavailable explicit default: fall back to the first available provider and
  show a small note such as `Default provider Codex is not installed. Using Claude.`
- No available providers: disable `Initialize`/`Clone` and show a provider
  setup/readiness prompt

The backend must still reject missing providers at spawn time with a clear
message before provider bootstrap/session-id work begins. This covers Wardian
CLI requests, workflow agent nodes, restored agents, custom clone requests,
existing-agent provider changes, and provider availability changes between
readiness load and spawn. Workflow restore/re-spawn failures must be logged and
surfaced to the workflow run rather than silently discarded.

## Consequences

- **Positive**: Users see the local provider constraint before they try to
  launch an unavailable provider.
- **Positive**: The frontend can stop hardcoding Claude as the only default
  while preserving Claude-first auto behavior.
- **Positive**: CLI, workflow, clone, restore, and race cases receive clearer
  backend errors instead of low-level process launch failures.
- **Positive**: The readiness model can later grow optional diagnostics such as
  version/path display without changing the spawn contract.
- **Negative**: Users may still pass executable readiness but fail on auth,
  billing, first-run prompts, or workspace trust; those remain provider runtime
  failures with existing troubleshooting docs.
- **Negative**: Provider executable resolution must stay aligned with launch
  resolution, especially for Windows shims, npm global installs, and macOS GUI
  `PATH` behavior.
