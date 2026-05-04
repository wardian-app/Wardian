---
name: wardian-cli
description: Use when an agent needs to inspect Wardian agent identity, list peers, check live or persisted agent status, find workspaces, or use the Wardian command-line interface from inside a Wardian-managed process or local terminal.
---

# Wardian CLI

Use the `wardian` command to inspect Wardian agents from a terminal. Prefer it over guessing from UI state or filesystem files.

## Quick Start

```bash
wardian agent list --scope all
wardian agent list --scope all --fields name,class,provider,workspace,status
wardian agent <name-or-uuid>
wardian agent show <name-or-uuid>
```

`wardian agent` and `wardian agent show` without a target mean "show the current agent." They require `WARDIAN_SESSION_ID`, so they usually work only inside a Wardian-managed agent terminal.

From an ordinary terminal, pass a target:

```bash
wardian agent show Wardian-Codex
wardian agent show 019d331a-0500-7592-969f-8f437886f42b
```

## Output

Default output is indented JSON:

```json
{
  "schema": 1,
  "agents": [
    {
      "name": "Wardian-Codex",
      "uuid": "019d331a-0500-7592-969f-8f437886f42b",
      "class": "Coder",
      "provider": "codex",
      "workspace": "D:/Development/Wardian",
      "status": "idle"
    }
  ]
}
```

Use `--field` for shell-friendly bare values:

```bash
wardian agent Wardian-Codex --field status
wardian agent Wardian-Codex --field workspace
```

Use `--fields` to request only specific JSON fields:

```bash
wardian agent list --scope all --fields name,status
wardian agent list --scope all --fields name,status,status_source
```

`status_source` is hidden by default. Request it when you need to know whether the answer came from the running desktop app or persisted state:

- `live` means the running desktop app answered.
- `persisted` means the CLI fell back to `state.db`.

## Listing Agents

```bash
wardian agent list
wardian agent list --scope all
wardian agent list --scope all --status idle
wardian agent list --scope all --class Coder
wardian agent list --workspace D:/Development/Wardian
```

Default list scope is `workspace` when the caller is a Wardian agent with a known workspace; otherwise it falls back to all agents. Use `--scope all` when you need the full roster.

## Show Agent

```bash
wardian agent <name-or-uuid>
wardian agent show <name-or-uuid>
wardian agent show <name-or-uuid> --fields name,class,provider,workspace,status
wardian agent show <name-or-uuid> --pretty
```

## Errors

Errors are JSON on stderr. Common cases:

- `not_in_session`: self lookup was requested outside a Wardian-managed process. Pass an explicit name or UUID.
- `not_found`: the requested name or UUID was not found. Run `wardian agent list --scope all --fields name,uuid`.
- `db_unavailable`: no live app answered and `state.db` was unavailable. Open Wardian or set `WARDIAN_HOME` to the expected home.

## Development Notes

When testing a dev app and CLI together, set the same `WARDIAN_HOME` in both terminals. The live control endpoint is keyed by `WARDIAN_HOME`.

PowerShell:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
npm run dev
```

Second terminal:

```powershell
$env:WARDIAN_HOME = "$PWD\.tmp\wardian-cli-dev"
cargo run -p wardian-cli -- agent list --scope all --fields name,status,status_source
```

After a release build from this workspace, use repo-root `target` outputs:

```powershell
D:\Development\Wardian\target\release\Wardian.exe
D:\Development\Wardian\target\release\wardian-cli.exe agent list --scope all
```
