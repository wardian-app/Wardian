# Queue v2 Action Needed Alerts

## Purpose

Queue v2 turns the Queue from a passive completion inbox into a triage surface for human-in-the-loop work. This slice adds the first live HITL queue event, lets users filter visible event types from the Queue header, and lets users opt into desktop or sound alerts per event type from Settings.

## Event Types

Queue cards are grouped by these event types:

- `action_needed`: an agent changed from another known status into `Action Needed`.
- `agent_completed`: an agent changed from an active status back to `Idle`.
- `workflow_completed`: a workflow run finished successfully.
- `workflow_failed`: a workflow run finished with failed status.

`workflow_failed` is a filter and alert key derived from a `workflow_completed` queue item whose status is `failed`. This keeps persisted queue items compatible with the existing workflow completion record while giving failures their own triage controls.

## Preferences

Queue preferences are stored under the active Wardian home at:

```text
<wardian-home>/queue/preferences.json
```

The document stores three boolean maps keyed by event type plus a sound volume:

- `visible_event_types`
- `desktop_notifications`
- `sound_notifications`
- `sound_volume`

All event types are visible by default. Desktop and sound alerts default to `true` only for `action_needed`; every passive completion/failure event defaults to `false` for alerts until the user opts in. `sound_volume` defaults to `0.5` and is clamped to the `0..1` range when preferences are loaded or changed.

Visibility filters live in the Queue header because they affect the current triage list. Desktop alert rules, sound alert rules, and sound volume live in **Settings > Queue** because they are notification policy rather than per-review workflow controls.

## Runtime Behavior

The frontend listens to the same agent status streams that already drive completion queue items. A new `action_needed` card is created only when Wardian has a previous status for the agent and then observes `Action Needed`, avoiding stale startup hydration cards.

Action-needed cards use the amber warning treatment used elsewhere for action-required status. Agent cards can focus the related agent terminal from Queue. When the action-needed summary contains recognizable provider choices, such as numbered options, the card renders those choices as compact buttons and submits the provider token directly. Wardian does not infer generic yes/no buttons from approval-looking text and does not keep a generic freeform Queue response box because approval intent should come from an explicit provider option or from the live terminal.

When the app receives a generic `Action Needed` status but has buffered recent provider text for the agent, the queue card uses that buffered text as the action-needed summary and then clears the buffer. This keeps numbered approval choices visible in Queue without reusing the approval prompt later as a completion summary.

Desktop notifications use Wardian's native desktop notification plugin when available, with the WebView notification API as a fallback. Sound alerts use a short Web Audio tone at the configured queue sound volume. If either capability is blocked by OS or browser policy, queue item creation still succeeds.

## Testing

Unit and component tests cover:

- preference defaults, merge behavior, mutation, and persistence calls
- action-needed queue item creation and deduplication
- workflow failure filter classification
- Queue rendering, header filtering, Settings notification rules, terminal open action, and clickable action choices
- App status-transition integration from `Processing...` to `Action Needed`
- notification dispatch defaults, native-notification fallback behavior, and sound volume scaling for action-needed events
