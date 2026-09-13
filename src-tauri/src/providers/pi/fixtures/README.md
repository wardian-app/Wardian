# Retained Pi request provenance fixture

`real-headless-session.jsonl` is a sanitized copy of a real Pi 0.84.2 session
written by the native headless automation conformance harness on 2026-09-07.
The selected model was `openai-codex/gpt-5.4-mini`. The original session file was
`2026-09-07T09-40-03-213Z_e1e33694-c782-423f-9142-bf974206a195.jsonl`, beneath
`<isolated-wardian-home>/agents/<temporary-provider-agent>/pi/sessions/`.
Its original SHA-256 is
`d3bca351d6319bdae523dd47c1d1836fae9b75a4af30e9f61797a5e7569d331f`.

Sanitization replaces the session working directory with a placeholder and
removes `responseId`, `thinkingSignature`, and `textSignature`. Entry IDs,
parent IDs, message roles, content, timestamps, and model fields are retained.
The user request entry has ID `ec1b3195` on the outer record and no nested
message ID; the assistant entry has ID `1f0e1517`. The user's parent is a
thinking-level configuration entry, so it must not become the request root.

The regression reads this actual fixture through `normalize_chat_lines` and
also reads the user record alone to prove that its root survives bounded log
tail loading. The fixture proves normalization of a recorded real turn, not
successful automation lifecycle completion or a fresh paid-provider retest.
This turn performed no tools. Supplementary tool records in the unit test are
explicitly synthetic and verify that entry IDs do not overwrite tool-call IDs;
they do not establish real-provider tool acceptance.
