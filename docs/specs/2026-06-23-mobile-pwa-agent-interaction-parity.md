# Mobile PWA Agent Interaction Parity

## Context

Wardian's mobile PWA is a remote command surface for the desktop app. It should preserve the agent interaction paths that matter when the user is away from the desktop: watchlist triage, agent detail inspection, chat, approvals, selected actions, broadcast prompts, and completion review.

The desktop remains the authoritative runtime for PTY state, provider lifecycle, and persisted queue data. The PWA should expose remote controls only through gateway-backed APIs or data already streamed by the gateway.

## Decisions

- The mobile watchlist keeps its first screen focused on agent status. Multi-agent broadcast is exposed through an explicit collapsed action and requires confirmation before dispatch.
- Mobile agent detail uses the same approval-choice parser and markdown copy behavior as desktop chat where the browser environment permits it.
- Provider slash-command style input is represented as an explicit command mode in the mobile composer and sent through the remote action endpoint as `input_mode: "command"`.
- Selected-agent clone is exposed in the mobile action strip and refreshes the remote roster after the backend accepts the clone action.
- The mobile Queue tab shows browser-local completion cards derived from streamed remote terminal output and status transitions. This is intentionally not durable queue storage; a future gateway queue endpoint should replace or hydrate this browser-local model.

## Current Scope

The parity audit is tracked in `docs/audits/mobile-pwa-agent-interaction-parity.csv`.

The implemented PWA parity slice covers:

- chat rendering, copying, approvals, and disabled-input states;
- selected-agent actions including clone;
- command-mode prompt dispatch;
- collapsed broadcast prompt dispatch with confirmation;
- compact activity rows for diff, todo, terminal, and changed-file evidence;
- mobile queue completion summaries from streamed OpenCode terminal output followed by an Idle status frame.

## Out Of Scope

- Durable remote queue persistence across PWA browser sessions.
- Full desktop queue preference management in the PWA.
- Native PTY or provider-runtime claims from browser E2E alone.

## Verification

Use the standard frontend and backend validation gates for this slice:

```bash
npm run lint
npm run test
npm run build
cd src-tauri && cargo check
cd src-tauri && cargo test
cd src-tauri && cargo clippy
```

PowerShell:

```powershell
npm run lint
npm run test
npm run build
Set-Location src-tauri
cargo check
cargo test
cargo clippy
```
