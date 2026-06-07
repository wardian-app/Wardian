# Workflow Engine — Trigger / Invoker Foundation (sub-project 6a) Design

- **Status:** Implemented
- **Date:** 2026-05-30
- **Part of:** [Workflow rework epic (#425)]; old-system parity track. Foundation for 6b (schedule invoker), 6c (monitoring sidebar), 6d (old workflow system deletion). Builds on 5a (`workflow_run`) + 5b (Run dialog).

> **Model (from brainstorming):** a *run* is an **instance** of a blueprint, like an **agent is an instance of a class**. The **blueprint** is the single source of truth for *behavior*; an **invoker** supplies the *context* of one invocation — `{ blueprint, input, bindings }`. Triggers aren't graph nodes that hold a mechanism; the mechanism lives in the invoker (manual now; schedule / file-watch / webhook later). The blueprint has one generic **entry node** that declares the input contract and is invocation-agnostic.

## 1. Goal & scope

Establish the **invocation contract** every invoker shares — a run parameterized by **`input`** (params, available graph-wide) and **`bindings`** (role→agent overrides) — and collapse the trigger node types into one generic entry. Ship **parameterized manual runs** (the old workflow system "runtime modal", generalized) on top of it.

**In scope (6a):**
1. Collapse trigger node types → one generic entry (`manual_trigger`, which already carries `input_schema`); remove `scheduled_trigger` + `file_watcher` node types.
2. Thread `input` + `bindings` through `workflow_run` → the engine + executor.
3. Render a **parameterized Run modal** from the entry node's `input_schema` in the unified view.

**Deferred:** persistent **invoker entities** (stored schedules/webhooks/file-watchers as their own records) — 6b builds the first (schedule) on this contract. Concrete file-watch / webhook invokers — later, same rails. A bindings *editor UI* beyond the minimal case — folded into 6b's invoker UI.

## 2. What already exists (leverage, don't rebuild)

- `manual_trigger` **already** declares an `input_schema` field (`FieldType::JsonSchema`) and is the engine's initial runnable node — it is the generic entry node.
- The engine **already** exposes the run's trigger payload graph-wide: `RunState::set_trigger(v)` writes `registry["trigger"]["output"] = v`; `RunStarted { trigger }` calls it; interpolation resolves `{{trigger.output.*}}` (tested in `engine/core.rs`).
- `Engine::start_with_id(bp, run_id, trigger, run_root, exec)` **already** takes a `trigger: Value` — 5a's `workflow_run` currently passes `json!({})`. The plumbing is there; 6a feeds it real input.
- 5a's executor resolves `role:`/`class:` agent refs (`resolve.rs`) — 6a adds a `bindings` override layer in front of it.

## 3. Changes

### 3.1 Registry — one entry node (`wardian-core`)
Remove the `scheduled_trigger` and `file_watcher` `NodeTypeDef`s from `workflow/registry.rs`. Keep `manual_trigger` as the single entry (id unchanged → the 16 migrated blueprints, which all use `manual_trigger`, keep working). Update its description to "Entry point; starts on demand or when an invoker fires it." Regenerate via `wardian workflow gen-schema` + `gen-docs` (CI drift guard enforces). The builder's palette/forms update automatically (registry-driven). *(Scheduling config does not live on this node — it lives on the schedule invoker in 6b.)*

### 3.2 Invocation contract — `input` + `bindings` (`src-tauri` + executor)
Extend the run commands:
```
workflow_run(path, provider?, workspace?, input?: Value, bindings?: Map<String,String>)
```
- **`input`** → passed as the engine `trigger` (replacing `json!({})`), so it lands at `registry["trigger"]["output"]` and is referenceable anywhere as `{{trigger.output.*}}`. (Bonus: this lets the 5c-migrated blueprints' stripped `{{trigger.output.*}}` references be restored.)
- **`bindings`** (role→target, e.g. `{"reasoning_gate": "class:Researcher"}`) → carried into the `LiveStepExecutor`; `resolve_agent` consults `bindings` first: if a task's `agent` is `role:X`/`class:X` and `bindings` has `X`, resolve to the bound target; else fall back to today's default resolution. This is old workflow system's `role_mappings`, now a per-invocation binding owned by the invoker — not the blueprint.

`workflow_resume` carries the same `input`/`bindings` for the resumed run (read back from the run's `RunStarted` event so a resume reuses the original invocation context). Same extension on the CLI `exec` verb (optional `--input <json>` / `--bind role=target`) for parity.

### 3.3 Parameterized Run modal (`src` / 5b)
In the unified view's **Run** flow (`RunLaunchDialog`), after the blueprint is loaded, read the entry node's `input_schema`. If non-empty, render an input form (one control per declared param, typed from the schema) below the existing provider/workspace fields. On confirm, collect the values into `input` and pass them to `workflow_run`. If `input_schema` is empty, the dialog is unchanged. (Role→agent `bindings` entry in the manual modal is optional in 6a — minimal/absent; the rich bindings UI ships with 6b's invokers.)

## 4. Components & files (indicative)
```
crates/wardian-core/src/workflow/registry.rs   # remove scheduled_trigger + file_watcher
src/features/workflows/nodeRegistry.schema.json # regenerated (gen-schema)
docs/workflows/node-reference-workflow.md             # regenerated (gen-docs)
crates/wardian-core/src/engine/executor.rs      # AgentTaskRequest unchanged; bindings applied in resolve
src-tauri/src/workflow/resolve.rs            # resolve_agent honors bindings
src-tauri/src/workflow/runs.rs               # thread input + bindings into the executor + engine trigger
src-tauri/src/commands/workflow.rs              # workflow_run / workflow_resume gain input + bindings
crates/wardian-cli/src/...                      # exec --input / --bind (parity)
src/features/workflows/RunLaunchDialog.tsx      # param form from entry input_schema
```

## 5. Testing
- **Unit (wardian-core):** registry no longer contains `scheduled_trigger`/`file_watcher`; engine: a run started with a non-empty `trigger` resolves `{{trigger.output.X}}` in a downstream node's field (extend the existing interpolation test).
- **Unit (executor/resolve):** `resolve_agent` with a `bindings` map maps `role:X` → the bound target; without a binding, falls back to default.
- **Frontend (RTL):** `RunLaunchDialog` renders fields from a non-empty `input_schema` and passes the collected `input` to `invoke('workflow_run', …)`; renders nothing extra for an empty schema.
- **Browser E2E (5b):** open a blueprint whose entry declares an input param → Run → fill the param → assert `workflow_run` got the `input`; the run observes as before.
- **Integration (src-tauri, mock provider):** `workflow_run` with `input` + a `bindings` map → the run's `RunStarted` records the trigger; a `{{trigger.output.*}}`-referencing node resolves; a `role:`-bound task resolves to the bound target. (Reuses 5a's mock-provider harness.)
- Registry drift: `gen-schema --check` + `gen-docs --check` pass.

## 6. Risks / notes
- **Non-breaking entry rename:** keeping the node id `manual_trigger` avoids touching the 16 migrated blueprints; only its meaning/description generalizes. Removing `scheduled_trigger`/`file_watcher` is safe — no shipped blueprint uses them (migration converted all triggers to `manual_trigger`).
- **`input_schema` shape:** it's a JSON-schema field; 6a renders a pragmatic form (string/number/bool/enum) from it rather than a full JSON-Schema form builder — richer types are a follow-up. The contract (params declared on the entry, supplied at invocation, referenceable graph-wide) is the durable part.
- **Bindings scope:** 6a delivers the *resolution* layer + command surface; persistent per-invoker bindings + their editor arrive with 6b's schedule invoker, which is the first thing that needs to store them.
- **Sub-workflows:** a `sub_workflow` node passing `input` to a child uses the same contract (child entry `input_schema` ← parent-supplied `input`). Wiring the `sub_workflow` node to pass input is confirmed-compatible here but implemented where sub_workflow execution lands (not 6a if sub_workflow isn't yet executed by 5a).
