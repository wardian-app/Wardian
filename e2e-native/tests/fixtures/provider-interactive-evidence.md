# Interactive approval and retained cost evidence

These new suites belong to #1170. They do not change the existing chat, context,
native broker, rendering, or New Session tests. `clear_session` retains its
existing New Session meaning; no new view-clearing feature or matrix row exists.

## Real approval suite

`provider-interactive-actions-real-native.test.mjs` is manual and requires an
explicit single provider, coordinator-verified usable model, version, prebuilt
artifact, and local observed-terminal profile. It performs two intentional
requests: reject a scratch write, then approve a different scratch write. It
never retries an uncertain submission. The provider must expose a current
approval for the exact file, action-required status, a native tool call and a
linked native outcome. A missing native projection is a gap, not a pass from
terminal text or an absent sentinel alone.

The profile is JSON with these required string fields:

| Field | Evidence required |
| --- | --- |
| `provider`, `provider_version` | Exact selected provider and observed installed version |
| `evidence` | Private reference to the retained observation establishing this profile |
| `ready_text` | Text identifying the ready composer |
| `prompt_text` | Exact current tool-approval prompt label, not workspace trust/model choice |
| `deny_choice`, `allow_choice` | Exact displayed rejection and approve-once choice labels |
| `deny_keys`, `allow_keys` | Navigation/choice plus Return, observed for that version |

No genuine approval profile is bundled: no permitted native observation was
available to this sidecar. The deterministic strings are explicitly synthetic.
The coordinator must supply the observed profile before a paid run. The suite
checks visible labels and operation before sending its constrained keys. It does
not interpret profile contents as shell commands, regular expressions or code.

Claude manual, Codex on-request/read-only with explicit `gpt-5.4-mini` and low
reasoning effort, and Antigravity explicit permission mode are configured only
on newly created disposable agents. Installed Codex 0.153.4 rejects `untrusted`;
the replacement is a parser-supported discovery candidate, not an observed
tool-approval pass. The exact observed profile and current native call remain
required before any choice or acceptance assertion. No product policy is remapped.

OpenCode remains blocked until a disposable tool-permission policy is verified.
Pi's installed native CLI has no built-in manual tool-approval mechanism; project
trust is a different operation. Extension-based approval may be possible, but
this suite installs no extension and uses no dangerous stimulus to force a prompt.
Pi therefore remains explicitly blocked. A source adapter
that lacks denial/call correlation will fail the assertion; do not relax it to
count assistant claims as successful permission enforcement.

Run deterministic checks without a native app or provider:

```sh
node --test e2e-native/tests/provider-interactive-evidence.test.mjs e2e-native/tests/provider-cost-evidence.test.mjs
```

PowerShell uses the same command. Real opt-in environment variables are listed
at the top of the manual suite. The coordinator owns serialized execution.
Private profiles, terminal snapshots and identities remain in the newly created
retained test home; only sanitized report fields may enter the public ledger.

## Cost reader

`provider-cost-retained.json` contains genuine OpenCode 1.18.29 role/time/cost
field extracts from the retained 2026-09-07 real run, with IDs replaced
consistently. Four completed assistant messages report cost zero; the retained
Wardian snapshot has only the first three ingested turns. The regression must
detect that missing resumed turn even though both sums are zero. Adding the
fourth telemetry row in a test is explicitly a synthetic positive case.

`readRetainedCostEvidence` accepts exact `provider`, `nativePath`,
`providerSessionId`, `statePath`, and `wardianSessionId` arguments. It reads
SQLite with read-only connections, binds the telemetry source to the exact
agent/provider/real path, then proves native session ownership before comparing
unique native turn sets and values. A Pi per-file session header is authoritative
when telemetry's provider-session ID is null; contradictory non-null metadata
fails. An OpenCode database source can span fresh sessions: its latest metadata
ID is not a per-turn owner. Every selected telemetry turn must map uniquely to a
completed native assistant message and an existing native session. Only proven
other-session rows are excluded. Native costs and ownership use one DB snapshot.
It never launches a provider, migrates a database, refreshes telemetry or guesses
the newest conversation. A reader invocation against a retained home is not a
fresh real-provider run.

`provider-cost-session-binding-retained.json` adds genuine sanitized v7 evidence:
Pi has a valid header, null metadata and three native accounting records versus
two ingested rows. OpenCode has one source key with three original-session and
two fresh-session ingested turns; its current native snapshot has three original
and four fresh completed messages. The original three-turn cohort passes exact
zero-cost parity. Pi and the fresh OpenCode session still fail for their missing
native turns after binding is repaired. These snapshots do not establish the
cause of that ingestion gap. No intersection with ingested IDs may hide it.
Synthetic mutations/completions exercise rejection and positive branches
separately from those genuine observations.

Pi supports native `usage.cost.total` and response IDs; no full retained Pi
parity pass is claimed. Claude/Codex return
explicit unavailable native cost, with no fabricated zero or ingestion pass.
Antigravity accounting and retired Gemini retain their intentional exclusions.
Cost ingestion/availability is separate from cost display and invoices. No
model-price arithmetic is performed. The reader fails on incomplete/duplicate
turns, missing source binding, malformed costs, or zero/null conflation.
