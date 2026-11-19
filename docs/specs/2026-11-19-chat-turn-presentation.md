# Chat turn presentation

Status: proposed for review. Issue: [#1752](https://github.com/wardian-app/Wardian/issues/1752).
Depends on [#1751](https://github.com/wardian-app/Wardian/issues/1751).
This PR specifies behavior; implementation follows separately after review.

## Decision and evidence

Present each turn as one prompt, one collapsible work summary, and an answer.
The answer has the strongest visual emphasis. Expanding work reveals ordered
steps and intervening narration; output belongs inside its step.

Current source evidence:

- `AgentChatView.tsx` consumes `useChatPages`, then calls
  `derivePresentedChatRows` and `withTurnChangeSummaries`.
- `workLogPresentation.ts` groups adjacent work events. Assistant prose breaks
  those groups; a work group is therefore not a turn.
- `chatTurns.ts` splits on user messages because provider `turn_id` is optional
  and inconsistent. Its file counts are agent-reported, not Git verification.
- `ChatTranscriptRows.tsx` has separate work-group, activity, and detail
  expanders. Work entries expose titles and summaries; detail continuations
  can appear outside the owning step.
- The Rust `conversation_archive` projection already owns bounded reads,
  canonical identities, aliases, revisions, and detail references. See
  [bounded Chat](2026-10-05-bounded-chat-projection.md),
  [recent publication](2026-11-07-chat-recent-publication.md), and
  [packed pages](2026-11-12-packed-chat-recent-pages.md).

The supplied reference survey motivates a turn summary like Claude/ChatGPT,
and T3 Code's per-item, lazily mounted, bounded output. This is design input,
not a claim of independently tested behavior in those products.

Prefer producer-owned turn membership over extending adjacency grouping:
pagination and narration must not change which turn owns a step. Keep the
existing projection and reader; do not introduce a second transcript store.
Provider execution, permission policy, and archive evidence retention are out
of scope. This supersedes the default command-first row presentation in
[the one-line tool spec](2026-08-24-chat-tool-call-one-line.md); exact commands
remain available in step details and Copy command.

## Turn anatomy

1. **Prompt:** render the committed user message once, including attachments.
   A local submission indicator belongs to the composer until committed; it
   must not become an optimistic transcript row.
2. **Work:** one summary for a turn containing steps or narration. Initially
   collapsed, including while live. Example: `Working · 4 steps · Reading config`
   or `Worked for 2m · 6 steps · Edited 2 files (reported)`.
   Omit unavailable duration/counts rather than inventing them. A turn with no
   work has no empty work expander.
3. **Steps and narration:** expanding the summary reveals one chronological
   list, with output initially closed for every step.
4. **Answer:** render provider-designated final text outside work using the
   existing Markdown renderer, normal assistant alignment, and primary text.
   Stream it in the same stable answer slot when its phase is known. Keep
   existing copy, links, attachments, and change-summary actions.

The work header uses muted text and a quiet border. Do not wrap the answer in
another activity card. Keep one turn-level change summary/action after the
answer; its reported-change provenance must remain explicit. The summary's
file count refers to that same evidence, not a second aggregation.

## Steps, narration, and state

Each step header contains a category icon, tool name, one human description,
status text/icon, and an expander only if detail exists or is pending. Examples:

| Tool | Human description | Optional result summary |
| --- | --- | --- |
| Shell | Run frontend tests | 42 passed; exit 0 |
| Read | Read package.json | 86 lines |
| Browser | Inspect open tabs | 3 tabs |
| Edit | Update chat styles | 2 files reported |
| Tool | Run custom_tool | Result available |

Descriptions come from bounded, deterministic tool adapters using invocation
intent, safe basename/command context, and known fields. Never use raw output,
serialized arguments, provider lifecycle labels, or arbitrary JSON as a title.
Unknown tools fall back to `Run <tool name>`; output summaries fall back to
`Result available`. Input/output remain inspectable inside details. No extra
model call is required to label work.

Narration uses muted assistant prose in the expanded chronological list,
between the relevant steps, aligned consistently with the answer. Preserve
paragraphs; it is neither a tool step nor counted as one. While collapsed,
the header may show the latest short step description, never a raw narration
or output excerpt. Do not duplicate narration next to the summary. Do not
expose hidden provider reasoning.

Use existing theme tokens throughout: `--color-wardian-text-muted` and current
card/border tokens; status tokens below, with text and icons as well as color.

| State | Display | Token |
| --- | --- | --- |
| Running | Working / Running | `--color-wardian-processing` (cyan) |
| Completed | Worked / Done | `--color-wardian-success` (emerald) |
| Needs permission | Action required | `--color-wardian-warning` (amber) |
| Failed | Failed | `--color-wardian-error` (red) |
| Cancelled, incomplete, unknown | Stopped / Incomplete / Unknown | `--color-wardian-off` (gray) |

Completion comes from turn/step evidence, never agent idle status alone.
A failed step stays failed even if a later retry succeeds; a real retry is a
new step. A transport replay updates the existing step. Cancellation ends the
spinner and preserves partial output. Interruption without terminal evidence
shows `Incomplete`, not success. A completed turn can say `Worked · 1 failed step`.

On permission requests, expose the existing actionable approval control directly
below the summary even while collapsed, keyed to its owning step and shown
only once. Failure gets a visible summary count and a shortcut to its step.
Do not auto-expand or collapse work on completion or refresh, or steal focus.
Preserve explicit expansion choices through compatible revisions; clear them
when conversation scope changes. A turn ending without final text shows
`Stopped before an answer`, `Turn failed`, or `No final answer recorded` as
appropriate, without fabricating an answer.

## Output and long turns

- One `Show output`/`Hide output` control per step opens input, concise result
  summary, and output in that step. Mount/fetch detail only when opened. No
  orphan links and no nested `Show full details` or `Read more` ladder.
- Output viewport: at most 20rem (`max-h-80`) and the available viewport height;
  vertical scrolling, wrapped text, and contained horizontal scrolling for
  code/JSON. Opening begins at the retained output's bottom. Live output follows
  bottom only while the reader remains there; scrolling up pauses following
  and offers `Jump to latest`. Do not move the outer transcript scroll.
- Producer returns at most the last 16 KiB of output per open step request,
  on a UTF-8 boundary, plus truncation/completeness metadata. Display
  `Earlier output not shown` above a truncated tail. Never concatenate detail
  continuations into unbounded browser state. An explicit `Open full output`
  action uses the existing inspection surface/artifact when available; it is
  not another in-transcript expansion. Preserve archive data and privacy rules.
- Known JSON results show an allowlisted key-field summary, e.g. `3 tabs`.
  Pretty-print only inside opened detail, only for a complete bounded JSON
  value. A truncated or invalid value stays escaped text with its marker;
  never parse a clipped tail as if complete. Copy output copies the displayed
  retained output and identifies truncation; full export is a separate action.
- Diffs remain closed initially. Up to 200 changed lines across at most three
  files use the same bounded detail viewport; larger diffs offer `Open changes`
  in the existing Changes surface. No automatic large diff panel.
- The collapsed summary remains constant height regardless of step count.
  Expanded work shows a scrollable list capped at 24rem or half the viewport,
  whichever is smaller. Virtualize beyond 50 entries; fetch older steps only
  through the projection's bounded cursors. Keep narration in order and retain
  an explicit `Earlier steps` control for unloaded history, distinct from output
  truncation. Counts cover the turn; label incomplete counts `at least N steps`.
  Expanding work never loads all history or opens output automatically.
- Keep the user's scroll anchor when history or new steps arrive. Retain the
  existing transcript follow-bottom behavior only when already near the bottom.

At widths below 640px, desktop grid Chat and Remote Chat share the same model
and renderer. Keep prompt/work/answer order, compact spacing, and one-line
ellipsized descriptions with accessible full labels. Move duration and reported
file count to a second header line when necessary; never hide status, approval,
or the step count. Use 44px touch targets and keyboard-operable buttons with
`aria-expanded`/`aria-controls`. Output is capped at 40vh and list at 50vh;
neither causes page-wide horizontal overflow. Announce state changes politely,
not every output chunk, and honor reduced motion.

## Projection contract

The Rust projection producer is authoritative for membership, pairing,
deduplication, ordering, labels, aggregates, and phases. Both clients render
one committed projection; refresh/delta application is revision reconciliation
within that source, never a union of provider logs, delivery records, local
submissions, recent previews, and persisted rows. A non-authoritative recent
preview may report loading progress, but supplies no narrative rows in this
presentation. This deliberately trades preview immediacy for one authority.

Add a versioned presentation contract to existing bounded page/detail APIs
and matching snake_case Rust/TypeScript DTOs. Required logical fields follow;
these are semantics, not a prescribed storage layout or a new public API.

| Record | Required information |
| --- | --- |
| Page | presentation version; existing session/conversation/generation/source epoch, revision, reset, aliases, removals, bounded cursors/progress |
| Turn | stable `turn_key`, prompt reference(s), status, ordered answer references, duration and reported-change evidence, step count and completeness |
| Item | stable `item_key`, owning `turn_key`, immutable order key, kind (`step`, `narration`, `answer`), evidence references |
| Step | tool identity/category, human description, status, optional summary, invocation/result references, detail availability/reference |
| Detail | owning step, bound revision/scope, retained text/format, pending/unavailable/complete flags, truncated-before flag, total size when known |

Invariants:

1. Keys survive refresh, restart, result arrival, and Older paging. Scope keys
   include conversation identity; never derive identity from display text,
   timestamps, array positions, or the currently loaded page boundary. Producer
   aliases explicitly migrate a provisional display identity when necessary.
2. Resolve request/turn membership and call/result links in the producer using
   proven source identities. Provider `turn_id` alone is insufficient. For
   legacy data, use the archive's accepted request boundary; if attribution is
   unproven, emit one stable `Earlier activity` group with partial membership,
   not an invented prompt or a merge into the next turn. Concurrent calls keep
   distinct keys; an unpaired result is an honest standalone step.
3. A call and its results occupy one step. Replayed evidence is idempotent;
   identical text from genuinely separate requests stays separate. Turn totals
   and order come from the producer, not the loaded subset. Older pages may
   extend one existing turn without emitting another summary or prompt.
4. Use provider final/commentary markers where available. Without a reliable
   phase, preserve `Assistant message` in work; after proven termination the
   producer may designate the last assistant message as the answer only when
   source ordering establishes it. Unknown phase stays unknown. No client
   heuristic promotes narration into an answer because the agent went idle.
5. Publish dependent presentation headers and references before the committed
   head changes. Preserve existing 80-header, 256 KiB IPC and reader budgets;
   a large turn spans pages. Build summaries incrementally, never by scanning
   a whole turn on a normal read. `partial`/pending data remains visibly partial.
6. Reset, alias and retraction semantics remain those of `useChatPages` and
   the row-revalidation contract. Detail must pass existing scope/revision and
   privacy checks. A stale reference is renewed through the committed owner;
   unavailable output gets an inline retry, never a raw-source fallback.
   Failed refresh retains the last admitted view with a stale indicator.

The load-bearing implementation work is durable turn membership and bounded
turn aggregates, which current `AgentChatEvent` pages do not expose as this
contract. Styling the existing adjacency groups alone cannot satisfy it.

## Transition and acceptance

1. Land #1751's immediate defect fixes independently. Add producer contract and
   fixtures behind an explicit version capability; preserve legacy reader DTOs.
   Rebuild derived presentation metadata incrementally through the existing
   projection owner. Leave source evidence untouched; checkpoints resume after
   interruption and never block recent reads on complete history.
2. Switch the shared renderer only for supported versions. Old clients retain
   the existing reader; new clients use corrected legacy Work log rendering
   when capability is absent. Each view selects exactly one representation.
   Unknown versions fail to legacy presentation with a diagnostic, never mix.
3. Cut over after conformance tests below and desktop/mobile screenshots. On
   failure, disable the new renderer and retain additive projection metadata.
   Remove the legacy grouping path only after shipped client compatibility and
   saved-history migration are verified; do not delete archive evidence.

Implementation acceptance (not claims made by this spec-only PR):

| Lowest useful layer | Required evidence |
| --- | --- |
| Producer/unit | Replayed prompt appears once; parallel calls, result-before-call, real retries, missing IDs, legacy partial history, multiple answers, and phase ambiguity preserve ownership and order. Refresh/restart/Older keep identities, totals, aliases and removals. |
| Reader/unit | Bounded tail and JSON behavior; truncation marker; privacy, stale detail renewal, pending/unavailable detail; large turn never exceeds existing budgets or forces an archive scan. |
| Shared React/unit | One summary per turn despite narration; no raw JSON titles or orphan expanders; lazy detail mounting; final answer emphasis; approval reachable collapsed; cancelled/incomplete states stop spinning; expansion survives refresh. |
| Browser | Normal desktop and narrow Remote Chat flows; 100+ steps, huge output, failed/retried tool, live final answer, scrolling up during output, Older across a turn, reconnect, keyboard/touch, light/dark/reduced motion. Capture changed states for implementation PRs. |
| Native/remote integration | Same committed projection through Tauri and authenticated remote reader; provider capture/restart and permission actions preserve real lifecycle behavior. Mock UI evidence alone cannot prove this. |

Coder handoff after spec approval: producer membership/contract and bounded
detail tail first, shared presentation second, migration/compatibility removal
last. Measure native incremental-publication cost and remote scroll behavior
in implementation; the size thresholds above are product defaults, not measured
performance claims. Public Chat guides and screenshots change with implementation.

## Mockups

```text
Completed, collapsed
You       Fix the settings layout.
Agent     > [done] Worked for 2m · 6 steps · Edited 2 files (reported)

          Fixed the layout. The controls now wrap at narrow widths.
          Tests: 42 passed.
          [Copy answer] [View changes]

Live, expanded (icons represented by text)
You       Fix the settings layout.
Agent     v [working] Working · 4 steps · Reading config
          [file] Read · Read package.json                  Done  >
          I will check the responsive rules next.
          [edit] Edit · Update chat styles                 Done  >
          [shell] Shell · Run frontend tests               Running v
            42 passed
            [Earlier output not shown]   <-- above retained tail
            +---------------------------------------------+
            | retained output; independent bounded scroll |
            +---------------------------------------------+
          [shield] Shell · Install dependency     Action required >
          [Approve] [Deny]  (visible even when work is collapsed)

Mobile, completed
You    Fix the settings layout.
Agent  > [done] Worked · 6 steps
         2m · Edited 2 files (reported)

       Fixed the layout. The controls
       now wrap at narrow widths.
       [Copy answer] [View changes]
```
