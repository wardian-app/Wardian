# Retained archive boundary fixtures

`real-pi-session.jsonl` is the sanitized real Pi 0.84.2 recording used by #1167.
The native headless automation harness recorded it on 2026-09-07 with
`openai-codex/gpt-5.4-mini`. Its original session filename was
`2026-09-07T09-40-03-213Z_e1e33694-c782-423f-9142-bf974206a195.jsonl` under
`<isolated-wardian-home>/agents/<temporary-provider-agent>/pi/sessions/`.
Original SHA-256:
`d3bca351d6319bdae523dd47c1d1836fae9b75a4af30e9f61797a5e7569d331f`.

Sanitization replaces cwd and removes responseId, thinkingSignature and
textSignature. Entry IDs, parent IDs, roles, content and timestamps remain.
The request's native envelope ID is `ec1b3195`; the assistant's is `1f0e1517`.
The boundary test reads the real fixture through the normalizer, represents
the pre-fix missing ID, and supplies #1167's current envelope mapping as the
archive input. It does not claim a fresh adapter or paid-provider retest.
This turn performed no tools.

`real-agy-delivered.json` extracts the first two stored events and their shared
narrative from a real Antigravity 1.1.27 interactive native chat run on
2026-09-07 using `gemini-3.6-flash-low`. The generated input and the native
step-0 database event already share one narrative's `event_refs`. The native
row has `provider_log: true`, but the old active merge hides it.

Original events.jsonl SHA-256:
`0b239e0748e3e9349df6bb5bc4eb8f9e2e8a03b23c54a701046816f410d5e141`.
Original conversation.jsonl SHA-256:
`3a36bbefd58986bde970760cab51c84de75732e71f476b0a343b85761f96a163`.
Sanitization replaces the Wardian session ID with `agent-1` and the native
source path with `<provider-data>/<native-session>.db`. Opaque event IDs,
reference links, prompt, sequence, and source metadata remain as recorded.
This old observation does not report metadata.source; no source-4 value is
invented for it. Separate source-4/source-2 regression inputs explicitly model
#1169's adapter output. Tool projection and legitimate assistant commentary
are outside this archive fixture's assertions.

## Antigravity mutable tool completion

`real-agy-tool-completion.json` contains the actual archived running event and
narrative plus the completed boundary DTO projected from a read-only query of
the same retained integration-v1 SQLite step. Cascade
`1b7cc6a3-7c7d-43ce-9d22-fdd8e6362a25`, step 2, type 132, source 2, native
status 3. Result text is the exact UTF-8 payload at 140.2.1; the retained archive
had native status 2. The full original payload SHA-256 is recorded in the
fixture. Only relevant extracted fields are retained; opaque payload fields
and local paths are excluded. The log path is replaced by an isolated fixture
path consistently on both observations; original event ID and narrative
sequence/time/references remain. The probe is disposable synthetic file data.
This tests the shared archive boundary, not a substitute provider adapter.
