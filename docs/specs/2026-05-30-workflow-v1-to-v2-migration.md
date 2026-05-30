# Workflow Engine v2 — v1→v2 Migration (sub-project 5c) Design

- **Status:** Proposed
- **Date:** 2026-05-30
- **Part of:** [Workflow Engine v2 epic (#425)]; follows 1–5b. **Migration only** — v1 code stays; full v1 deletion is deferred until v2 reaches parity for v1's still-live features (remote control, scheduled triggers, monitoring sidebar).

> **Operational task, not shipped code.** Per the architecture spec, v1→v2 migration is a one-time operational/agent-assisted re-authoring, not a permanent importer. The deliverables are: (a) v2 `.md` blueprints in the user's `~/.wardian/library/workflows/`, and (b) a migration report committed to the repo. No code in `src/` or `src-tauri/` changes.

## 1. Goal & scope

Convert **all 16** existing v1 workflow JSON definitions (`~/.wardian/workflows/wf-*.json`) into validated v2 `.md` blueprints under `~/.wardian/library/workflows/`. v1 JSONs are left intact (untouched) so any conversion can be redone.

**The 16 (v1 file → v2 slug id):**

| v1 file | name | v2 slug |
|---|---|---|
| wf-00heartbeat | Passive Heartbeat | `passive-heartbeat` |
| wf-1774982619713 | Heartbeat | `heartbeat` |
| wf-nexus-sdlc | Nexus SDLC | `nexus-sdlc` |
| wf-trident-alerts | Trident Alerts | `trident-alerts` |
| wf-trident-csp-daily | Trident CSP Daily Report | `trident-csp-daily` |
| wf-trident-daily-strategy | Trident Daily Strategy Report | `trident-daily-strategy` |
| wf-trident-free-data-refresh | Trident Free Data Refresh | `trident-free-data-refresh` |
| wf-trident-leaps-scan | Trident LEAPS Scan | `trident-leaps-scan` |
| wf-trident-orb15-scan | Trident ORB15 Scan | `trident-orb15-scan` |
| wf-trident-pure-csp-scan | Trident Pure CSP Scan | `trident-pure-csp-scan` |
| wf-trident-regular-scan | Trident Regular Scan | `trident-regular-scan` |
| wf-1773202498672 | Sync Laduc | `sync-laduc` |
| wf-1773303609664 | Loop Test | `loop-test` |
| wf-1773370765255 | Auto-Fix Audit | `auto-fix-audit` |
| wf-1773496511889 | Summary | `summary` |
| wf-1774167797429 | Ralph Wiggum | `ralph-wiggum` |

## 2. Naming convention (v2)

- **`id`** = descriptive **kebab-case slug** derived from the v1 `name` (table above). It is the filename (`library/workflows/<id>.md`), the run-dir key (`logs/workflows/<id>/<run-id>/`), and the CLI/reference key (`wardian workflow exec <id>`). Terminal-safe, readable, no timestamps.
- **`name`** = the original human title, kept verbatim for display.
- **Uniqueness** is the filesystem's job — one `<id>.md` per directory. For these 16 all slugs are distinct; flat layout in `library/workflows/` (organize into subfolders later if desired).
- (Tooling note, not part of 5c: `workflow write` / the builder save should auto-suffix on collision — a small future enhancement.)

## 3. Node-type mapping (v1 → v2)

v1 node types in use: `agent`, `communication`, `command`, `trigger`, `memory`, `logic`, `loop`, `script`, `schedule`. Map to the v2 registry (`wardian workflow node-types --json` is the live contract; `docs/workflows/node-reference-v2.md` + `building-workflows.md` are the references):

| v1 type | v2 type | notes |
|---|---|---|
| `agent` | **task** | `decision` instead when the node branches on a constrained choice (v1 `json_schema` decision / multiple outgoing ports keyed by choice) |
| `command` | **shell** | command string → shell `command` |
| `script` | **script** | runtime + path |
| `memory` | **state** | read/merge/write storage → `state` op |
| `logic` | **branch** (boolean) or **decision** | condition → `branch`; multi-way → `decision` |
| `communication` | **task** (or **notify**) | inter-agent send → a `task` whose prompt performs the send, or `notify` for a pure notification |
| `loop` (back-edge) | **loop** container | v1 back-edge loop → v2 loop container node owning a `body` subgraph (`parent` field); `max_iterations`/`until` from v1 `settings`/loop config |
| `trigger` / `schedule` | **manual_trigger** | ⚠️ **lossy — v2 has no scheduler yet.** A scheduled v1 trigger (e.g. heartbeat every 6h) becomes a `manual_trigger`. Each converted blueprint records the original cadence in its description (e.g. "was: interval every 360 min") so re-scheduling is trivial once the v2 scheduled-trigger sub-project lands. |

Ports/pulse → v2 typed ports; v1 `dependencies[].port` edges → v2 `edges` with `from_port`/`to_port`. v1 `role_mappings` / `config.role`/`agent_id` → the v2 task `agent` field (prefer `role:<role>` or `class:<class>`; a concrete `agent_id` maps to a `role:`/`class:` ref where possible, else the agent name).

## 4. Procedure (per workflow, ×16)

1. Read the v1 JSON (`~/.wardian/workflows/<file>.json`).
2. Author a v2 blueprint `~/.wardian/library/workflows/<slug>.md` (YAML front-matter `schema: 2`, `id: <slug>`, `name: <original name>`, nodes/edges per the §3 mapping), consulting `docs/workflows/node-reference-v2.md` + `wardian workflow node-types --json`.
3. **Validate:** `wardian workflow validate ~/.wardian/library/workflows/<slug>.md` → must report `ok: true` (no error diagnostics). Iterate until clean.
4. **Smoke-run:** `wardian workflow exec ~/.wardian/library/workflows/<slug>.md --executor mock` → must reach a terminal state (parses + drives end-to-end under the mock executor). This catches structural problems the validator alone won't.
5. Record the per-workflow outcome (slug, node-type mapping notes, any lossy conversions like scheduling, validate/exec result) in the migration report.

## 5. Deliverables

- **16 validated v2 blueprints** in `~/.wardian/library/workflows/*.md` (operational; on the user's machine).
- **Migration report** committed to the repo at `docs/workflows/v1-migration-report.md`: the slug table, the node mapping applied, per-workflow notes, and a prominent list of **scheduled→manual** conversions that need re-scheduling once v2 scheduling lands.
- v1 JSONs untouched; no `src/` or `src-tauri/` changes.

## 6. Verification (definition of done)

- Every one of the 16 blueprints passes `wardian workflow validate` (ok, no error diagnostics) **and** `wardian workflow exec --executor mock` (terminal state).
- The report lists all 16 with results and flags every lossy conversion.

## 7. Risks / notes

- **Scheduled→manual is lossy and unavoidable now** (v2 has no scheduler). Mitigated by recording the original cadence in each blueprint's description + the report. The Trident suite + heartbeats are likely the scheduled ones most affected — those workflows won't auto-run until a future v2 scheduled-trigger sub-project; they're runnable manually (`wardian workflow exec`) meanwhile.
- **Agent references:** v1 nodes reference agents by `agent_id`/`role`. v2 prefers `role:`/`class:` refs (5a runs them headless). Where a v1 node pins a specific live agent, the conversion uses the closest `role:`/`class:` ref and notes it; exact-agent routing is a deferred v2 feature anyway.
- **Mock-exec is a structural smoke, not a behavioral guarantee** — it proves the blueprint parses and the engine can drive it, not that the (deferred-real-executor) agent steps do the right thing. Behavioral correctness is validated when the workflows are run for real later.
- **Fidelity vs. cleanup:** the conversion preserves v1 intent rather than redesigning; obvious v1 cruft (single-node test workflows like `loop-test`, `ralph-wiggum`) is converted faithfully, not improved.
