# Provider function conformance

This matrix tracks observed Wardian behavior for each provider. It complements
the [product feature comparison](./agent-development-environment-feature-matrix.md)
with real-provider acceptance evidence. Work is tracked in
[issue #1159](https://github.com/wardian-app/Wardian/issues/1159).

The [function matrix CSV](./provider-function-matrix.csv) compares providers
side by side. The [evidence CSV](./provider-function-evidence.csv) records the
baseline and latest result, provider version, model, Wardian revision and build,
execution mode, case, acceptance condition and evidence for each cell. These
are views of recorded observations, not a provider capability declaration.
The [observation ledger](./provider-conformance-observations.jsonl) retains the
individual results behind those views, including earlier failures and corrected
harness prerequisites. Timestamped retests supersede earlier observations for
the same required scenario. For example, ordinary resume cannot clear a failure
of resume after New Session. A passing result from an older harness remains in
the ledger, but does not qualify a changed assertion without a matching rerun.

## Status meanings

| Status | Meaning |
| --- | --- |
| Pass | The exact acceptance assertion passed using the real provider on the recorded build. |
| Fail | Reproduced behavior contradicts the expected contract. |
| Reported failure | A reported symptom still needs independent reproduction. |
| Blocked | An external dependency or harness prerequisite prevented a result. |
| Untested | No qualifying real-provider assertion has run for this behavior. |
| Design skip | An intentional support exclusion has an explicit reason. |

An absent observation is never a pass. A passing launch or delivery test does
not establish chat refresh, provenance, usage, skills or session continuity.
Provider output must be attributed to an assistant response before an echoed
prompt marker can count as a successful response. Unit tests and simulated
provider fixtures establish local contracts, not real-provider acceptance.

## Scope and exclusions

The maintained provider set is Claude, Codex, OpenCode, Antigravity and Pi.
Gemini remains visible as a design exclusion because its maintenance was
[explicitly discontinued](https://github.com/wardian-app/Wardian/issues/581).
That exclusion makes no claim that a particular Gemini function works or fails.

Antigravity token usage and cost logging are intentionally unsupported. Its
archive-derived activity history remains in scope. Missing accounting must
remain unreported rather than zero. Other blocked or untested functions must
not be relabeled as intentional omissions merely to complete the matrix.

The current audit runs on Windows. A pass establishes behavior only for the
recorded provider version, model, mode, build and platform; it does not imply
macOS or Linux acceptance. Interactive, native transport and automation paths
have separate observations where they exercise different behavior.

The matrix includes five separate messaging assertions: background task and
correlated reply, background conversation continuity, exchange through the
original terminals, information delivery without a new turn, and interruption
of an observed active turn. Codex observations imported from the retained
stock-provider acceptance reports identify their original source, artifact and
harness hashes. They do not qualify a later integration build or supersede
the separate legacy delivery, secret-recall or interruption-and-recovery cases.

## Model selection and evidence

Tests use the cheapest available usable model per provider. Live catalog
discovery precedes execution, because hardcoded model defaults can disappear.
Free and local routes require a successful access check. Opaque aliases or
subscription billing prevent a conclusive price comparison; such selections
are recorded as lowest-known-cost candidates with the limitation stated.

Provider versions and selected models belong alongside run evidence. A model
catalog entry alone does not prove account access or a completed prompt. A
provider-side rate limit or unavailable local server is a blocker for that run,
not proof of an adapter defect. When a fallback is necessary, its selection
basis and the failed lower-cost candidate remain visible in the evidence.

The existing real-provider entrypoints are the native delivery, rendering and
headless automation tests under `e2e-native/tests/`. Their opt-in boundaries are
documented in [Real Provider Test Boundary](../specs/2026-05-28-real-provider-test-boundary.md).
Baseline and retest observations identify the built artifact independently of
the checkout that invokes it. Reusing an older executable cannot verify a fix.

Public evidence contains synthetic test content and repository-relative
references. Raw provider logs, credentials, personal paths and live-agent
identifiers do not belong in these CSV files or linked issue comments.

## Recorded snapshot — 2026-09-09 canonical harness

The [spreadsheet](../../outputs/provider-conformance-20260909-final1170/provider-function-matrix.xlsx)
and CSV files contain **72 Pass, 21 Fail, 17 Blocked, 97 Untested and 45 Design skip** cells
across 42 functions and six providers. All **803 observations** from the previous
refresh are preserved unchanged. No new provider run was imported.

The previous refresh contained 77 Pass, 19 Fail, 16 Blocked, 95 Untested and
45 Design skip cells. Selecting the final native broker suite makes five historical
passes ineligible. Earlier failures and blockers remain visible when a newer
observation cannot qualify. This change does not assert that product behavior
regressed. Candidate 3 and later artifacts have not been qualified by this export.

The five retained Codex messaging passes identify their original build13 artifact.
Model and cost metadata retain the earlier discovery date and limitations.
Antigravity usage and cost remain explicit design exclusions. Historical results
are not evidence of a new build or a different operating system.

Final harness delivery is tracked in [issue #1170](https://github.com/wardian-app/Wardian/issues/1170).

| Area | Function | Claude | Codex | OpenCode | Antigravity | Pi | Gemini |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Discovery | Model catalog refresh | Pass | Pass | Pass | Pass | Pass | Design skip |
| Discovery | Direct model access | Pass | Pass | Pass | Pass | Pass | Design skip |
| Discovery | Interactive model choice | Untested | Fail | Untested | Untested | Untested | Design skip |
| Lifecycle | Launch and readiness | Pass | Blocked | Pass | Pass | Fail | Design skip |
| Delivery | Short prompt | Pass | Blocked | Pass | Pass | Fail | Design skip |
| Delivery | Multiline prompt | Pass | Blocked | Pass | Pass | Untested | Design skip |
| Delivery | Trailing newline | Pass | Blocked | Pass | Pass | Untested | Design skip |
| Delivery | Long pasted prompt | Pass | Blocked | Pass | Pass | Untested | Design skip |
| Delivery | Completion and idle status | Pass | Blocked | Pass | Pass | Fail | Design skip |
| Delivery | Delivery acknowledgement | Pass | Fail | Pass | Pass | Fail | Design skip |
| Chat | Provider session identity | Pass | Blocked | Pass | Pass | Untested | Design skip |
| Chat | Current chat-log link | Pass | Blocked | Pass | Pass | Untested | Design skip |
| Chat | Live transcript refresh | Pass | Untested | Pass | Pass | Untested | Design skip |
| Chat | Genuine user prompts | Pass | Fail | Pass | Pass | Fail | Design skip |
| Chat | Injected context roles | Untested | Blocked | Untested | Untested | Untested | Design skip |
| Chat | Request and turn correlation | Pass | Blocked | Pass | Pass | Fail | Design skip |
| Chat | Assistant response deduplication | Pass | Untested | Pass | Pass | Untested | Design skip |
| Chat | Tool calls and results | Pass | Blocked | Pass | Pass | Fail | Design skip |
| Chat | Durable archive replay | Pass | Untested | Pass | Pass | Blocked | Design skip |
| Lifecycle | Pause and resume | Pass | Blocked | Fail | Pass | Untested | Design skip |
| Lifecycle | Fresh session | Pass | Fail | Pass | Pass | Untested | Design skip |
| Lifecycle | Clear session | Untested | Untested | Untested | Untested | Untested | Design skip |
| Terminal | Terminal rendering and resize | Fail | Untested | Blocked | Untested | Untested | Design skip |
| Terminal | Terminal scrollback | Untested | Untested | Untested | Untested | Untested | Design skip |
| Telemetry | Token usage logging | Pass | Untested | Pass | Design skip | Untested | Design skip |
| Telemetry | Activity history | Untested | Untested | Untested | Untested | Untested | Design skip |
| Telemetry | Cost reporting | Untested | Untested | Untested | Design skip | Untested | Design skip |
| Context | Instructions and workspace | Untested | Fail | Untested | Untested | Untested | Design skip |
| Context | Managed skill discovery | Untested | Untested | Untested | Untested | Untested | Design skip |
| Permissions | Approval handling | Untested | Untested | Untested | Untested | Untested | Design skip |
| Permissions | Automation approval rejection | Untested | Untested | Untested | Untested | Untested | Design skip |
| Headless | Ephemeral automation | Pass | Pass | Pass | Pass | Pass | Design skip |
| Headless | Fresh inherited automation | Fail | Untested | Fail | Blocked | Untested | Design skip |
| Headless | Resumed automation | Fail | Untested | Fail | Untested | Untested | Design skip |
| Native delivery | Native broker delivery | Untested | Fail | Fail | Blocked | Fail | Design skip |
| Native delivery | Native session continuity | Untested | Untested | Untested | Untested | Blocked | Design skip |
| Native delivery | Cancellation and recovery | Untested | Untested | Untested | Design skip | Untested | Design skip |
| Messaging | Background task and correlated reply | Untested | Pass | Untested | Untested | Untested | Design skip |
| Messaging | Background conversation continuity | Untested | Pass | Untested | Untested | Untested | Design skip |
| Messaging | Original terminal task and reply | Untested | Pass | Untested | Untested | Untested | Design skip |
| Messaging | Information without a new turn | Untested | Pass | Untested | Untested | Untested | Design skip |
| Messaging | Active turn interruption | Untested | Pass | Untested | Untested | Untested | Design skip |
