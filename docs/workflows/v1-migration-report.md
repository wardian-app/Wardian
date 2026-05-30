# v1 → v2 Workflow Migration Report

**Date:** 2026-05-30 · **Sub-project:** 5c (migration only; v1 code retained) · Spec: `docs/specs/2026-05-30-workflow-v1-to-v2-migration.md`

All **16** v1 workflow JSONs (`~/.wardian/workflows/wf-*.json`) were converted to v2 `.md` blueprints in `~/.wardian/library/workflows/`. Every blueprint passes both gates — `wardian workflow validate` (ok, no error diagnostics) **and** `wardian workflow exec --executor mock` (terminal `completed`) — independently re-verified after conversion. The v1 JSONs are untouched, so any conversion can be redone or diffed.

Conversion was performed by Sonnet subagents (a scout established the format; three converters split the rest), each gated per-workflow by the CLI.

## Results (all 16)

| v2 slug | v1 source | name | validate | exec (mock) | has loop | scheduled (now manual) |
|---|---|---|---|---|---|---|
| `summary` | wf-1773496511889 | Summary | ok | completed | — | — |
| `loop-test` | wf-1773303609664 | Loop Test | ok | completed | ✓ | every 1 min |
| `passive-heartbeat` | wf-00heartbeat | Passive Heartbeat | ok | completed | — | every 360 min |
| `heartbeat` | wf-1774982619713 | Heartbeat | ok | completed | — | every 6 h |
| `nexus-sdlc` | wf-nexus-sdlc | Nexus SDLC | ok | completed | ✓ | — |
| `sync-laduc` | wf-1773202498672 | Sync Laduc | ok | completed | — | weekly Mon–Fri 12:00 + 60 min |
| `auto-fix-audit` | wf-1773370765255 | Auto-Fix Audit | ok | completed | — | — |
| `ralph-wiggum` | wf-1774167797429 | Ralph Wiggum | ok | completed | ✓ | — |
| `trident-alerts` | wf-trident-alerts | Trident Alerts | ok | completed | — | — |
| `trident-csp-daily` | wf-trident-csp-daily | Trident CSP Daily Report | ok | completed | — | weekly Mon–Fri 15:30 |
| `trident-daily-strategy` | wf-trident-daily-strategy | Trident Daily Strategy Report | ok | completed | — | weekly Mon–Fri 09:45 |
| `trident-free-data-refresh` | wf-trident-free-data-refresh | Trident Free Data Refresh | ok | completed | — | daily Mon–Fri 08:15 + weekly Sat 09:00 |
| `trident-leaps-scan` | wf-trident-leaps-scan | Trident LEAPS Scan | ok | completed | — | weekly Mon–Fri 09:35 |
| `trident-orb15-scan` | wf-trident-orb15-scan | Trident ORB15 Scan | ok | completed | — | weekly Mon–Fri 09:55 |
| `trident-pure-csp-scan` | wf-trident-pure-csp-scan | Trident Pure CSP Scan | ok | completed | — | weekly Mon–Fri 09:35 |
| `trident-regular-scan` | wf-trident-regular-scan | Trident Regular Scan | ok | completed | — | weekly Mon–Fri 09:35 |

## Naming convention

Each blueprint's `id` is a descriptive kebab-case slug (= filename = run-dir key = CLI `exec <id>` arg); `name` keeps the original human title. Uniqueness is the filesystem's job (one `<id>.md` per directory). Flat layout in `library/workflows/`.

## Node-type mapping applied (v1 → v2)

`agent` → **task** (or **decision** for constrained-choice branching) · `command` → **shell** (`cmd`→`command`, `folder`→`cwd`) · `script` → **script** · `communication` → **notify** · `memory` → **state** · `logic` → **branch**/**decision** · `loop` (back-edge) → **loop** container (body nodes carry `parent: <loop-id>`; `body`/`done` ports) · `trigger`/`schedule` → **manual_trigger** · v1 `dependencies[].port` → v2 `edges`. Agent refs (`config.role`/`agent_id`/role_mappings) → `role:`/`class:` refs.

## ⚠️ Lossy conversions — review before real runs

1. **Scheduling lost (every scheduled workflow).** v2 has no scheduler yet (deferred from 5a), so all scheduled triggers became **manual_trigger**. The original cadence is recorded in each blueprint's markdown body (see the "scheduled" column above). These run on demand (`wardian workflow exec <id>` / the Run button) until a future **v2 scheduled-trigger sub-project** lands; then they can be re-activated. **Most affected: the Trident suite + both heartbeats + sync-laduc.**

2. **Runtime interpolations simplified to pass the mock gate.** v2 interpolation is *strict* (an unresolved `{{...}}` errors the step), and the **mock executor returns empty step outputs** — so references to upstream runtime outputs can't resolve under `--executor mock`. The converters therefore removed/simplified some of them, e.g.:
   - `trident-pure-csp-scan`: dropped a `{{nodes.trigger-pm.output.timestamp}}` command arg (cosmetic; the 4 python scan commands are preserved verbatim).
   - `trident-alerts`: simplified a final notify that referenced `{{nodes.command-1.output.stderr}}` / `exit_code`.
   - `nexus-sdlc`, `ralph-wiggum`: removed `{{storage.*}}` / loop-iteration references from messages (v2 doesn't yet expose loop-iteration context).
   - Each affected blueprint documents what was removed in its markdown body.
   **These would resolve under the real executor** (a shell step returns `stdout`/`stderr`; an agent task returns its output). **Recommend:** for any workflow you'll run for real, diff the v2 blueprint against its intact v1 JSON and restore the interpolations you need — `validate` will still pass (it doesn't run interpolation); only `exec --executor mock` can't complete them.

3. **Minor:** per-node `timeout_ms` (v1) has no v2 equivalent and was dropped; loop `max_iterations` clamped to the v2 maximum where v1 exceeded it.

## Notes

- **The fidelity of structure + commands is high** — shell commands, webhooks, working directories, edges, and loop bodies are preserved as-authored. The simplifications are confined to runtime-output interpolations and scheduling, both flagged above and in each blueprint.
- **Process feedback for v2:** the `--executor mock` gate is a weak validator for interpolation-heavy workflows because mock step outputs are empty under strict interpolation. A richer mock (scriptable per-node outputs) or a lenient/"dry-run" interpolation mode would let future migrations keep runtime interpolations without manual stripping.
- **v1 retained.** No `src/` or `src-tauri/` changes; v1 engine, scheduling, remote control, and monitoring stay live. Full v1 deletion remains deferred until v2 reaches parity for those features.
