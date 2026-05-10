# Spec 035: Custom Agent Clone

- **Status:** Implemented
- **Date:** 2026-05-10
- **Decider:** User

## Context and Problem Statement

Wardian currently supports Fresh Clone and Profile Clone for single agents. Fresh
Clone copies visible setup while starting a clean provider conversation. Profile
Clone also copies a small fixed set of agent-local profile data: `AGENTS.md` and
instance-level `.agents/skills`.

Users need a custom clone flow that preserves Wardian's physical-first model
while allowing finer control over what crosses into the new agent. The motivating
case is changing provider engine during clone, but the same flow should also
allow changing name, class, workspace, selected agent-local files, and selected
agent-specific skills.

Custom Clone must not copy opaque provider memory or generated runtime state.
The Rust backend remains the authority for session identity, provider bootstrap,
filesystem safety, habitat projection, and clone registration.

## Proposed Decision

Extend the existing clone system instead of adding a parallel custom workflow.
Fresh, Profile, and Custom Clone should share the same sanitization and
registration path in `clone_agent`.

### User Experience

The single-agent context menu keeps the existing Clone submenu and adds a third
item:

- Fresh Clone
- Profile Clone
- Custom Clone

Selecting Custom Clone opens a modal. The modal initializes from Profile Clone
defaults and creates the clone immediately when submitted, matching existing
Fresh/Profile behavior.

The modal contains:

- Identity fields: agent name, provider engine, agent class, and workspace path.
- Agent Files: a recursive checkbox tree of eligible source agent files.
- Agent Skills: source agent-specific deployed skills, selected by default.
- Actions: Cancel and Clone.

Agent-specific skills are keep/remove only in the first version. The modal does
not add arbitrary library skills; users can add more skills later through the
existing Library or agent configuration UI.

Changing provider preserves shared and still-applicable config fields. Runtime
identity fields are still cleared by clone sanitization regardless of provider:
`session_id`, `resume_session`, `fresh_provider_session_id`,
`codex_cleared_provider_sessions`, `system_include_directories`, and
`opencode_port`. Provider-specific preference fields such as model, sandbox,
approval, profile, tool, or agent settings may remain persisted, but only the
selected provider's runtime path should consume its own fields.

### Backend Preview Command

Add a preview command, likely `get_agent_clone_preview`, that returns:

- Source agent basics: name, provider, class, workspace.
- Suggested clone name.
- Eligible recursive file tree from `WARDIAN_HOME/agents/<source_session_id>/`.
- Default selected file paths matching Profile Clone behavior.
- Source agent-specific deployed skill refs from `.agents/skills`.
- Default selected skill refs matching Profile Clone behavior.

The preview is advisory. The backend must revalidate every submitted path and
skill at clone time.

### Eligible File Rules

The recursive file tree includes user-authored files under the source agent
directory and excludes generated or automated material.

Always exclude:

- `habitat/`
- `.agents/skills/` because skills are customized in a separate modal section.
- Provider session/history roots such as `.codex/`, `.claude/`, `.gemini/`, and
  `.opencode/` when they appear under the agent directory.
- Logs, telemetry files, permission request logs, and provider bootstrap output.

Preview traversal must not follow symlinks, Windows junctions, or other reparse
points. If a candidate path is a link/reparse point, show it as ineligible or
omit it from the tree. For every displayed and submitted file, canonicalize both
the source agent root and the candidate path and require the candidate to remain
inside the source agent root after resolution. This prevents external-file
exposure and recursive traversal loops.

`AGENTS.md` is selected by default when present. Empty eligible file lists and
missing `AGENTS.md` are valid states.

Directories are UI grouping only. The submit payload sends selected relative file
paths, not directory paths.

### Clone Request Shape

Extend `CloneAgentRequest` with a custom profile selection, for example:

```ts
profile_selection?: {
  files: string[];
  skills: DeployedSkillRef[];
}
```

When `profile_selection` is absent, existing Fresh/Profile behavior remains
unchanged. When present, `clone_agent` copies only the selected eligible files
and recreates only the selected source agent skills.

Existing override fields continue to apply:

- `session_name`
- `provider`
- `folder`
- `agent_class`
- `start`

`start` defaults to `true` for consistency with current clone actions.

For Custom Clone, an omitted `session_name` means "use the suggested/generated
clone name," matching existing Fresh/Profile clone behavior. The UI should
prefill the suggested name and reject a deliberately blank submitted input, but
the backend should continue accepting omitted names for generated-name flows.

### Skill Copy Semantics

Selected skills are identified by the pair `{ name, source_path }`.
`source_path` is authoritative when present and lets the backend distinguish
duplicate skill names. At submit time, the backend reloads the source agent's
current deployed skill refs and verifies that each selected pair still matches a
currently deployed source-agent skill.

For source-path-backed skills, recreate the deployment from the library by
`source_path`. This keeps linked skills live and consistent with the existing
library deployment model.

For legacy copied skills without source identity, copy the deployed skill
directory from the source agent's `.agents/skills/<name>` directory.

If a selected source-path-backed skill no longer exists in the library at submit
time, fail clearly instead of silently copying stale content. If duplicate
deployed skill names exist and a selected skill has no `source_path`, match only
the exact deployed directory name and treat it as a legacy copied deployment.

### Validation and Error Handling

Preview failure blocks the modal and offers a close action. Submit failure keeps
the modal open with selections intact.

The backend rejects:

- Duplicate clone names.
- Invalid workspace paths, using the same validation path as spawn.
- Absolute file paths.
- Traversal paths.
- Paths that resolve outside the source agent directory.
- Symlink, junction, or reparse-point file selections.
- Runtime or generated paths even when manually submitted.
- Skill refs not currently deployed to the source agent.

File and skill changes between preview and submit are handled by submit-time
validation.

Custom clone must be transactional from the user's perspective. If validation,
file copy, skill recreation, provider session discovery, or registration fails,
the backend must not register a partial agent. It should remove any new or
provisional destination directory it created for the failed custom clone while
leaving all source-agent files untouched.

### Provider Session Ordering

Some providers use a Wardian-generated session ID before launch. Others discover
the final provider session ID during launch. Custom Clone should follow the
existing clone split explicitly:

- For providers with generated session IDs, create the final destination agent
  directory first, then copy selected files and skills into that directory before
  registration/launch context uses it.
- For providers whose final session ID is discovered at launch, create a
  provisional UUID directory, copy selected files and skills there, and resolve
  `system_include_directories` against that provisional session ID before
  calling provider bootstrap/session discovery.
- After the real session ID is known, copy the same selected files and skills
  from the source agent into the real destination directory, then remove the
  provisional directory if it differs from the real session ID.
- On any failure after provisional creation, remove the provisional directory.
  On any failure after final destination creation but before registration,
  remove the final destination directory if Wardian created it for this clone.

The backend should copy selected profile data from the source agent both times,
not move from the provisional directory, so final content is always derived from
the same validated source selection.

### Data Flow

1. User chooses Clone > Custom Clone.
2. Frontend calls `get_agent_clone_preview(source_session_id)`.
3. Backend builds the safe preview from active state and disk.
4. Modal initializes with Profile Clone defaults.
5. User edits identity fields and file/skill selections.
6. Frontend submits one `clone_agent` request with overrides and
   `profile_selection`.
7. Backend revalidates selections.
8. Backend creates the clone using the provider ordering rules above, copies
   selected profile files and skills into the applicable destination, sanitizes
   runtime state, starts the clone, registers it after the source agent, and
   emits normal update events.
9. Frontend refreshes agents and closes the modal.

### Testing

Backend tests should cover:

- Preview excludes generated/runtime paths and `.agents/skills`.
- Preview includes recursive eligible files and selects Profile Clone defaults.
- Custom clone copies only selected files.
- Custom clone rejects traversal and absolute paths.
- Custom clone rejects or omits symlinks, junctions, and reparse points.
- Custom clone recreates only selected instance skills.
- Custom clone validates skill identity by `{ name, source_path }`, handles
  duplicate names, fails missing source-backed library skills, and supports
  legacy copied skill fallback.
- Provider, class, workspace, and name overrides still sanitize runtime session
  fields including `session_id`, `resume_session`, `fresh_provider_session_id`,
  `codex_cleared_provider_sessions`, `system_include_directories`, and
  `opencode_port`.
- Providers with discovered session IDs use provisional directories, final
  directories, and cleanup exactly as specified.
- Failed custom clone does not register a partial agent and cleans created
  provisional/final directories.
- Existing Fresh/Profile clone behavior remains unchanged.

Frontend tests should cover:

- Clone submenu shows Custom Clone.
- Modal loads preview and initializes expected defaults.
- Preview failure shows a blocking modal error and close action.
- File tree check/uncheck changes submitted file selection.
- Skill check/uncheck changes submitted skill selection.
- Provider, class, workspace, and name edits are submitted.
- Submit failure keeps the modal open and preserves choices.

Browser E2E with the mock provider should cover opening Custom Clone, adjusting a
file or skill selection, submitting, and seeing the new agent appear. Native E2E
is required only for claims about real filesystem copy behavior or provider
launch behavior beyond backend command tests.

## Consequences

- **Positive**: Users can cherry-pick agent-local context instead of choosing
  only all-or-nothing Fresh/Profile modes.
- **Positive**: Provider conversion during clone becomes a first-class workflow.
- **Positive**: Filesystem safety stays backend-owned and testable.
- **Positive**: Existing Fresh/Profile commands keep their current behavior.
- **Negative**: The backend clone path becomes more complex because it must build
  previews, validate selections, and support partial profile copying.
- **Negative**: The first version intentionally does not add new library skills
  during clone, so some customization remains a follow-up action.
