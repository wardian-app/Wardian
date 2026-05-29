# Workflow Engine v2 — Live Executor (sub-project 5a) Design

- **Status:** Proposed
- **Date:** 2026-05-29
- **Part of:** [Workflow Engine v2 epic (#425)]; follows sub-projects 1 (library/registry), 2 (durable engine), 3a (builder), 3b (run view), 4 (CLI).

> **Structure note:** the v2 engine/workflow logic lives as modules in `wardian-core` (`wardian_core::engine`, `wardian_core::workflow`). The live executor lives in `src-tauri` because it owns the agent runtime. Read "executor" as a `src-tauri` impl of the `wardian_core::engine::StepExecutor` trait.

## 1. Problem & goal

The v2 durable engine (`wardian_core::engine`) executes a validated blueprint as a resumable run, but every side-effecting step goes through the dependency-inverted `StepExecutor` trait — and the only implementation today is `MockExecutor`. So v2 can author (builder), observe (run view), and mock-execute (CLI), but **nothing drives real agents**. v1's `workflow_engine` is still the only thing that actually runs a workflow against live agents.

**Goal of 5a:** a real `StepExecutor` (`LiveStepExecutor`) in `src-tauri` that drives live work, plus the Tauri commands to launch / resume / cancel a run and pause for approval. This makes v2 actually execute and is the prerequisite for retiring v1 (5c) and unifying the workflow UI (5b).

**Non-goal / explicitly deferred:** live named-agent routing (sending a step to an already-running interactive roster agent over its PTY), per-node git worktrees, scheduled/cron triggers, cross-run concurrency caps, and per-run agent session continuity. See §7.

## 2. Transport: build on `run_headless`, validate it fresh

`src-tauri/src/manager/headless.rs::run_headless_with_options` already spawns a provider in one-shot headless mode (codex `exec --json`, claude `--print`, opencode `run --format json`, antigravity `--print`, **`mock`**), reads its output, and **returns the agent's final response as a value**. It is **request/response, not a PTY** — so it never uses the `ask`/`reply` channels (whose headless behavior is unvalidated). This is the correct transport primitive for headless workers.

**Caveat (explicit):** v1's headless *orchestration* was buggy. We treat `run_headless` as the right building block but **not** as proven, and we do **not** copy v1's patterns. 5a builds a clean executor and validates the primitive end-to-end (mock provider integration test + a gated real-provider smoke). v1's `workflow_engine` headless code is a cautionary reference only.

## 3. Agent resolution (hybrid by reference)

A Task/Decision node's `agent` field resolves as:

- **`role:<x>` / `class:<x>` / `ephemeral`** → spawn a fresh **headless ephemeral worker** for the step via `run_headless` (the default path; this is all of 5a's agent execution). An `AgentConfig` is built from the class/role (provider, class instructions, workspace). Workers are **headless and ephemeral per node** by default; **not** shown in the roster, so a run doesn't flood the Command Center. (A visible/grouped opt-in is a later enhancement, not 5a.)
- **explicit live agent name** → would route to a running roster agent over its PTY. **Deferred** (§7) — that transport is the genuinely unvalidated one. In 5a, a named reference either runs headless under a derived config or returns a clear "live-agent routing not yet supported" error (decided in the plan).

## 4. `LiveStepExecutor` (the StepExecutor impl)

New module `src-tauri/src/workflow_v2/` with `LiveStepExecutor` implementing `wardian_core::engine::StepExecutor`. Each side-effecting trait method maps to a real action:

- **`run_agent_task`** — resolve `agent` → build `AgentConfig` → `run_headless_with_options(cwd, prompt, session, "json", provider)` → take the returned response → **extract structured output**: if `output_schema` is set, parse/validate JSON (trailing fenced ```json block or whole response); otherwise best-effort parse, falling back to `{"text": <response>}`. Returns `StepOutput(value)` stored at `nodes.<id>.output`.
- **`run_decision`** — same call, with the prompt appended: "Respond with exactly one of: `<choices>`." Parse the chosen port. If the answer is not a declared choice, **re-prompt once**; if still invalid, fail the node. Returns `ChosenPort`.
- **`run_shell`** — run `command` in the run workspace cwd via the existing shell util; return `{exit_code, stdout, stderr}`. (No sandbox — local, author-controlled; documented.)
- **`run_script`** — run `runtime` (`python`/`node`/`sh`) on `path` in the workspace; capture output like shell.
- **`notify`** — fire an app notification (`tauri-plugin-notification`) and write a run-log line.
- **`state_op`** — apply `op` (`set`/`merge`/`delete`) with `entries` to the run's `{{storage}}` in the engine `RunState` registry that interpolation reads. (Exact op set pinned in the plan against the engine's storage model.)

**Testability boundary:** the executor calls agents through a small internal `AgentRunner` trait (one method: run a prompt headlessly → response). The real impl wraps `run_headless`; unit tests inject a fake so they never spawn a provider. This keeps the executor logic (resolution, parsing, decision constraint) unit-testable in isolation.

**Workspace:** a run executes in one workspace — an optional blueprint field, else a sensible default (e.g. the run dir or library workspace). No per-node worktrees in 5a.

**Concurrency:** the engine's `drive` loop dispatches runnable nodes **sequentially within a run** (confirmed in `driver.rs`), so the executor never sees concurrent calls from one run. Multiple concurrent runs are allowed; a global cap is deferred.

## 5. Run lifecycle (Tauri commands)

- **`workflow_run_v2(path)`** — load + validate the blueprint (refuse to run if invalid), build a `LiveStepExecutor`, generate a `run_id`, set `run_root = paths::workflow_run_dir(bp.id, run_id)`, and drive `Engine::start_with_id` in a **background tokio task**. Returns the `run_id` immediately; the run proceeds async, writing `events.jsonl` + `state.json` that **Run View (3b) already observes live**.
- **Approval** — when the engine reaches `AwaitingApproval` it returns and the status persists. `workflow_approve_v2(id, run, granted, note)` resumes via `Engine::grant_approval` / `reject_approval` in a new background task.
- **`workflow_resume_v2(id, run)`** — resume an interrupted/paused run via `Engine::resume` (completed nodes skipped).
- **`workflow_cancel_v2(id, run)`** — abort a live run (cancel the background task; mark the run failed/stopped with a reason).
- **Crash recovery** — on app start, scan `logs/workflows/**` for runs still marked `Running` (their workers are gone) and mark them **interrupted**; Run View surfaces a **Resume** affordance that calls `workflow_resume_v2`. No auto-resume (no surprise re-execution of side-effecting steps at startup).

## 6. Testing

- **Unit** (`wardian-core` unchanged; tests in `src-tauri`): `LiveStepExecutor` with an injected fake `AgentRunner` — agent resolution, output/schema extraction, decision constraint + re-prompt, shell/script/state ops. No real spawn.
- **Integration (the real validation):** end-to-end run through real `run_headless` with the **`mock` provider** — seed a blueprint in a temp `WARDIAN_HOME`, invoke `workflow_run_v2`, and assert the run dir / `events.jsonl` / terminal `state.json`, and that the CLI `runs`/`run-show` (sub-project 4) read it back. This properly validates the headless primitive that v1 used incorrectly.
- **Real-provider smoke:** opt-in (one headless codex/claude task), gated like the other real-provider tests.

## 7. Scope boundaries (deferred to later sub-projects)

- **Live named-agent routing** (PTY send/await/capture against a running roster agent) — the unvalidated transport; its own follow-up.
- **Visible/grouped run workers** in the Command Center (5a workers are headless).
- **Per-node git worktrees** for isolation.
- **Scheduled / event triggers** — 5a is manual-run only (`workflow_run_v2`); triggers are later.
- **Per-run agent session continuity** (resume_session reuse across a run's nodes).
- **v1 retirement & UI unification** — sub-projects 5b (unified Workflows view, edit↔observe↔run) and 5c (v1→v2 migration, delete v1 engine).

## 8. Risks

- **`run_headless` correctness** under workflow load is unproven → the mock-provider integration test + real-provider smoke are the gate, not assumption.
- **Structured-output reliability** from free-form agents → schema-validated parse with a re-prompt for decisions and a `{"text": …}` fallback for tasks; brittle prompts are tuned during implementation.
- **Provider variance** (codex/claude/opencode/antigravity headless output shapes already differ in `run_headless`) → 5a targets the providers `run_headless` already normalizes; per-provider quirks handled there.
