# Library

Use `wardian library` to inspect and contribute reusable agent assets without
opening the desktop app:

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

Library workflow commands only author blueprint files. Use
`wardian workflow` commands for workflow-specific behavior.
