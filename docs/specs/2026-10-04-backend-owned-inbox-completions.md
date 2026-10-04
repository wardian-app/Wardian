# Backend-Owned Inbox Completions

## Problem

Sending a message to an agent and waiting for the turn to finish should leave
one Inbox card holding the agent's answer. Before this change, only Claude
cards were written by the backend ([Claude Inbox Turn
Completions](claude-inbox-turn-completions.md)). For every other provider the
desktop frontend received a bare `agent-turn-completed` event, re-read the
chat transcript, and wrote a card itself. Three defects followed:

- **Colliding identity.** The card ID came from the final transcript event's
  ID. Live watch events are numbered by their position in a bounded buffer
  (`<session>:<sequence>:event_msg`), so later turns reused the IDs of
  earlier, often dismissed, cards. The queue merge keeps the existing record
  for a known ID, so the new answer was silently dropped. On 2026-10-04 one
  Codex agent finished 8 user-requested turns with final answers and received
  one card, hours late; across four Codex agents, 4 of 73 completed turns
  produced a card.
- **A mounted window was required.** A turn that finished while the desktop
  frontend was reloading, or while only the mobile Inbox was in use, produced
  no card.
- **The answer was truncated.** Cards kept a 500-character summary, and
  **Show details** expanded only that summary.

## Decision

The backend owns every agent completion card. A card is written to the
queue, keyed by provider evidence for the turn, before any client is told
about it. The frontend applies the persisted card and never builds one.

| Provider | Completion source | Evidence ID |
| --- | --- | --- |
| Claude | Injected `Stop` hook | Claude `prompt_id` |
| Codex | Owned app-server `turn/completed` (status `completed`, `final_answer` text), and rollout `task_complete` (`last_agent_message`) | Codex turn ID |
| Other providers | Final assistant message of the transcript at the turn boundary | `answer:` + digest of session, request, and answer |

- The app-server turn ID and the rollout `task_complete.turn_id` are the same
  identity (verified against a persisted #1519 task binding). Both Codex
  sources therefore converge on one card, and either alone is sufficient:
  provisional and restored runtimes still have the rollout watcher.
- The Codex owner observer reads a bounded, sequenced log of finished turns
  instead of only the latest activity. A watch receiver sees only the latest
  observation, so two turns finishing between wake-ups would otherwise lose
  the first. The log belongs to one runtime's owner connection, so the
  observer starts at its beginning and also reports turns that finished
  before the observer task first ran.
- A resumed rollout is re-read from its start. Records timestamped before the
  current provider process launched are history and never become cards.
- Some providers reuse one `turn_id` for a whole thread, and transcript event
  IDs are not stable for every source, so the generic path hashes the request
  and answer instead. Repeating an identical request and answer in one session
  yields one card.
- The generic path polls the transcript for up to three seconds, because a
  transcript can trail the turn boundary. It keeps polling while tool
  activity follows the last assistant message (interim prose) or while the
  candidate is already a card (an earlier turn). It is fenced to the runtime
  that reported the boundary.
- `agent-turn-completed` has two forms: a turn boundary (no `inbox_item`)
  that refreshes turn-scoped views, and a card projection (with
  `inbox_item`). Claude now emits both, matching the other providers.
- A failed queue write is retried while the same agent runtime remains
  current. A runtime-generation fence rejects completions from a replaced
  runtime.
- Expanding a completion card shows the full `response_text`, on desktop and
  mobile.

## Non-goals

- Policy for which turns deserve a card is unchanged: every completed turn
  with a final answer, including peer-requested turns, produces one, as
  Claude already did.
- Claude `StopFailure` (for example, a usage limit) still produces no card;
  [#1492](https://github.com/wardian-app/Wardian/issues/1492) tracks a
  stronger Claude terminal verdict.

## Verification

- Rust unit tests cover card identity and idempotence, the summary bound,
  rejection of blank answers, Codex rollout parsing including history and
  unanswered turns, owner/rollout identity convergence, coalesced owner
  observations, and generic transcript selection, including thread-wide
  `turn_id` reuse, interim prose, and provider control commands, plus
  persistence idempotence and the runtime-generation fence.
- Frontend tests cover the two event forms, the absence of frontend-built
  cards, and full-answer expansion.
- A real-provider run in an isolated `WARDIAN_HOME` must show one card per
  completed Codex and Claude turn.
