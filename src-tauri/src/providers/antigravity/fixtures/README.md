# Antigravity SQLite tool observations

`chat-tools-1.1.27.json` contains selected fields from paired SQLite step payloads
and JSONL records captured by the real native chat harness on 2026-09-07 using
Antigravity 1.1.27 and Gemini 3.6 Flash (Low). The retained harness root state,
archive manifest, and provider trajectory metadata identified the same original
conversation, before the harness cleared it and created a fresh conversation.

This is a minimized field extract, not a complete database or transcript. It
retains the original seven step indexes, types, sources and status values.
Machine paths, scratch filename, probe contents and call ID are sanitized.
User wrapper metadata, timestamps outside result text, settings, signatures,
unrelated tool internals and the system message body are omitted. Protobuf
lengths are re-encoded after selecting and sanitizing fields. JSONL call argument
values retain their observed JSON-string representation; SQLite arguments are
typed JSON. No generated provider run is represented as an observation.

Verified field mapping:

| Step | SQLite layout | Paired JSONL |
| --- | --- | --- |
| 0, 4 | type 14, source 4, text 19.2 | USER_INPUT / USER_EXPLICIT |
| 1 | type 15, source 2, text 20.3 | PLANNER_RESPONSE / thinking |
| 1 | repeated 20.7: ID 1, name 2, JSON args 3 | tool_calls / run_command |
| 2 | type 132, source 2, output 140.2.1 | GENERIC / command output |
| 3, 6 | type 15, source 2, text 20.1 | PLANNER_RESPONSE / content |
| 5 | type 101, source 5 | SYSTEM_MESSAGE / SYSTEM |

Source is payload 5.3. Payload status 4 equals 3 on these completed steps.
Thinking is established by the paired field, not by counting assistant messages.
Other tool-result layouts, multiple calls in one planner step, missing source,
unknown values and malformed bytes are not observations from this run. Tests
that mutate these records explicitly exercise synthetic compatibility cases.
