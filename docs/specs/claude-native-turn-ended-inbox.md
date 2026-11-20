# Native Claude turn-ended Inbox observations

Issue: #1700. This adds turn-end awareness without asserting task success.

Claude's native mod API documents `turn.complete` as a turn-ended signal,
including interrupted turns. Its `answer` reason does not prove success: a
forced prevention of continuation can still receive that classification.
Keep the existing interactive successful-verdict guard in place.

## Evidence and delivery

Each native launch receives a passive `--plugin-dir` plugin beneath its own
Wardian agent directory. It changes no global plugin, provider settings,
authentication, prompt, permission decision, or terminal transport. It forwards
each observed event and middleware result unchanged.

Match one composer/bridge `prompt.submit` to the actual main-loop `turn.start`
text. Capture the immutable `turn.complete` answer, interruption flag, raw
reason, provider session ID and native turn ID. Exclude subagents, foreign
event origins, dropped submissions, unmatched or ambiguous prompt ownership,
unsafe IDs, changed provider sessions, and missing typed event fields. Repeated
identical pending prompts are an explicit observability gap; their ownership
must not be guessed from queue order.

The plugin retains a launch-scoped `$.store` outbox and retries writing its
receipt files. The backend requires complete validated JSON, a matching native
handshake, and the current runtime incarnation. It atomically admits a receipt
to disk under the agent lifecycle lock, persists its canonical Inbox item,
then acknowledges it. Only backend-admitted files are eligible for restart
replay. Unadmitted producer files from retired launches cannot create notices.
Receipt identities include Wardian session, launch, provider session and turn.
Read state and dismissal tombstones survive retries and stale desktop saves.

These notices use `agent_turn_ended` / `agent-turn-ended`. Unknown outcomes use
neutral styling and **Outcome unverified**; an explicit interruption uses
**Turn interrupted**. Empty captured answers remain empty. Expanding details
retains the outcome qualifier and shows the captured answer. Existing agent
notification preferences also govern these notices. No success watch,
`agent-turn-completed`, change snapshot or automation completion is emitted.

## Availability and proof boundary

Mods require a compatible Claude Code version and can be disabled by provider
rollout, trust, safe/bare mode, or organization policy. The per-launch
`coverage.json` remains unavailable until an actual provider handshake arrives.
A handshake proves the bridge loaded, not that every future turn will deliver.
Crashes before durable capture or backend admission and rejected ambiguous
observations remain gaps. Neither a timer nor silence proves a turn ended.

The JavaScript contract tests use explicit API doubles. Rust tests exercise
validation, admission-file parsing, identity and semantic classification. Those
tests do not prove the installed provider loaded the mod, that its current API
matches these calls, or that native turn events reached the Inbox. Native
acceptance must independently cover normal response, interruption, tools,
subagent exclusion, prompt provenance, restart replay and dismissal.

The always-run `generated_plugin_preserves_launch_binding_and_ownership` unit
test calls the real plugin producer. It checks generated manifest and module
bindings, the unavailable initial handshake state, separate launch and agent
directories, preservation of the prior module, and rejection of unsafe owner
IDs. Its PASS proves this producer contract.

The same test has an explicit installed-provider validation extension. Set
`WARDIAN_CLAUDE_PLUGIN_VALIDATOR` to the absolute Claude executable to run
`plugin validate --json --strict` against the actual generated plugin. Run the
test with `--nocapture` to retain the separate machine-readable
`WARDIAN_CLAUDE_PLUGIN_VALIDATION` result. An absent variable reports
`NOT_REQUESTED`; it does not bypass any producer assertions or prove provider
validation. An explicitly invalid executable, rejected plugin or invalid
validator response fails the test. The extension uses an isolated configuration
and sends no model prompt.

The provider's capability scanner requires helpers receiving `$` to be declared
at module scope. Installed-provider static validation proves packaging and
scanner compatibility only; it does not establish runtime callback delivery.

Primary contracts:

- [Claude mod turn events](https://code.claude.com/docs/en/plugins/mods/events#follow-a-turn)
- [Version-specific mod types](https://code.claude.com/docs/en/plugins/mods/create#get-type-definitions-for-your-version)
- [Mods API](https://code.claude.com/docs/en/plugins/mods/api#reach-files-processes-and-the-network)
- [Mod availability](https://code.claude.com/docs/en/plugins/mods/troubleshoot#check-whether-mods-can-load)
