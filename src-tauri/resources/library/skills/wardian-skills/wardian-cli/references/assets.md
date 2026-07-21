# Assets

Wardian distinguishes reusable Library entries from durable artifact versions.
Use the Library to author and deploy agent assets; use artifacts to present
work produced by a managed agent for review.

## Manage Reusable Library Entries

```bash
wardian library list --flat
wardian library list skills --flat
wardian library show prompts/review.md --content
wardian library read classes/Reviewer
wardian library create skills/review/planner --stdin
wardian library write prompts/review.md --file <prompt-file.md>
wardian library tags prompts/review.md --set review --set daily
wardian library deploy skills/review/planner --targets user:global,class:Reviewer,agent:<agent-id>
wardian library deployments skills/review/planner
wardian library deploy skills/review/planner --clear
wardian library orphans
```

Use section-qualified refs: `skills/<path>`, `prompts/<path>.md`,
`classes/<Name>`, or `workflows/<path>.md`. Prompts and workflows require the
`.md` extension. A skill is a directory containing `SKILL.md`; do not nest a
skill inside another skill. Default class files initialize on first access.

`deploy --targets` replaces the entire desired deployment set with the supplied
non-empty, deduplicated list. Never pass an empty variable as the target list.
Use `--clear` only when removing every deployment. Use `orphan delete` only
after the current scan still identifies that deployment as orphaned.

Library workflow commands only author blueprint files. Use `wardian workflow`
commands for workflow-specific behavior.

## Present Durable Artifacts

From a Wardian-managed terminal, present an authorized file under the agent's
primary workspace or an additional granted directory:

```bash
wardian artifact present ./report.md --title "Report for review"
wardian artifact show <artifact-id> --version <version-id>
wardian artifact review show <artifact-id> --latest
```

The desktop app must be running, and `WARDIAN_SESSION_ID` identifies the origin
agent. Re-presenting the same canonical path normally appends a version to the
active artifact thread; use `--new` for a distinct thread or `--artifact <id>`
to require an exact existing thread. Repeat `--address <comment-id>` for each
comment addressed by the new version.

`artifact present` returns durable artifact, version, and presentation IDs only
after the Workbench accepts the background-tab transaction. If UI delivery
fails, it reports the already-persisted artifact details rather than deleting a
version. `artifact show` can fall back to the on-disk artifact store when the
app is not running; `present` cannot, because authorization and UI delivery are
live runtime contracts.
