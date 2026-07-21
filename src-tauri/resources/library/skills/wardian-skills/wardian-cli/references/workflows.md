# Workflows

Use workflow commands to inspect, validate, normalize, and run workflow
blueprints:

```bash
wardian workflow node-types
wardian workflow validate <path-to-workflow.md>
wardian workflow parse <path-to-workflow.md>
wardian workflow normalize <path-to-workflow.md> --write
wardian workflow exec <path-to-workflow.md>
wardian workflow runs
wardian workflow run-show <blueprint-id> <run-id>
wardian workflow replay <blueprint-id> <run-id>
wardian workflow schedule list
```

`validate`, `parse`, `normalize`, `runs`, `run-show`, and `replay` are
disk-backed. `exec` and schedule actions that launch runs require the desktop
app for the same `WARDIAN_HOME`.
