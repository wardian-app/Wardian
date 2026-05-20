# Antigravity Provider

- **Status:** Implemented
- **Date:** 2026-05-20
- **Issue:** #314
- **Decider:** Wardian maintainers

## Context

Wardian already supports Gemini CLI, but Antigravity is a separate CLI surface exposed through `agy`. The two providers should remain independently selectable. Gemini CLI support stays in place for now, with a migration note because consumer/free Gemini CLI access is scheduled to cut off on June 18, 2026.

Antigravity's useful headless output can be written to provider state instead of stdout. Local and official CLI evidence show:

- executable: `agy`
- interactive flag: `--prompt-interactive`
- headless flag: `--print`
- resume flag: `--conversation <conversation-id>`
- context roots: repeated `--add-dir <absolute-path>`
- runtime options: `--sandbox`, `--dangerously-skip-permissions`, `--print-timeout <duration>`
- state root: `~/.gemini/antigravity-cli`
- transcript path: `brain/<conversation-id>/.system_generated/logs/transcript.jsonl`

## Decision

Add `antigravity` as the first-class provider id and `Antigravity` as the user-facing label. The adapter is separate from Gemini and uses `AGENTS.md` for Wardian role context.

Visible agents launch with:

```bash
agy --add-dir <wardian-context-root> --prompt-interactive
```

Headless workflow runs launch with:

```bash
agy --add-dir <wardian-context-root> --print "<prompt>"
```

PowerShell uses the same provider arguments:

```powershell
agy --add-dir <wardian-context-root> --print "<prompt>"
```

When resuming, Wardian passes:

```bash
agy --conversation <conversation-id> --print "<prompt>"
```

Wardian discovers Antigravity conversation identity from `~/.gemini/antigravity-cli/cache/last_conversations.json` or the newest conversation under `brain/`. It stores the provider conversation id in `resume_session`, watches the transcript JSONL for live status, and uses completed `MODEL` `PLANNER_RESPONSE` records as provider-adapted transcript text for `wardian agent watch`.

Hidden Wardian context roots are exposed to Antigravity through visible temp projections before Wardian passes them with `--add-dir`. Projection roots containing `.agents/skills` are materialized instead of linked directly so deployed Wardian skills do not remain nested junctions or symlinks back into hidden storage. Skill deploy/remove operations refresh live Antigravity projections, and library skill watch events refresh projected skill contents while the watcher is active.

## Consequences

- Gemini CLI behavior remains unchanged and selectable.
- Antigravity does not use Gemini's `--include-directories`, `--session-id`, output-format, or log assumptions.
- Empty `agy --print` stdout is not treated as proof of failure when the transcript contains the answer.
- Workflows can run Antigravity through headless `--print`; JSON mode flattens the transcript-derived response into Wardian's existing workflow output path.
- Real-provider validation is opt-in behind `WARDIAN_E2E_REAL_ANTIGRAVITY=1` because it depends on local Antigravity authentication and account setup.
