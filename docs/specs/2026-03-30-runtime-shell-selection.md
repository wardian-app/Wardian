# Runtime Shell Selection

- Status: Implemented
- Date: 2026-03-30
- Tags: runtime, pty, settings, cross-platform

## Context
Wardian previously mixed several process-launch models.

- Agent providers were attached directly to PTYs.
- Workflow shell-command nodes used OS defaults such as `cmd /C` or `sh -c`.
- Headless provider execution and session bootstrap used their own launch paths.

That made shell behavior inconsistent across the product and prevented users from intentionally choosing a shell profile such as Git Bash for both agent sessions and workflow command execution.

## Decision
Wardian now exposes a single runtime shell setting in Settings.

- The setting is dynamically populated from shells discoverable on the current operating system.
- Workflow shell-command nodes execute through the selected shell.
- Interactive provider PTY sessions execute through the selected shell.
- Headless provider execution and provider bootstrap execute through the selected shell.
- `Auto` selects the best discovered shell for the host OS.
- `Custom` allows an explicit executable path plus command arguments.

Provider binaries still keep their provider-specific arguments and environment variables. Wardian wraps the full provider invocation in the selected shell rather than rewriting provider semantics.

On Windows, shim handling is host-aware.

- PowerShell hosts invoke `.cmd` and `.bat` provider shims directly through PowerShell.
- POSIX-like hosts on Windows such as Git Bash or WSL route Windows shims through `cmd.exe /c` for compatibility.
- Claude now resolves the actual executable found on `PATH` instead of assuming `claude.cmd`.

## Consequences
Positive:

- Users can align agent sessions, workflow shell commands, and headless execution under one shell profile.
- Git Bash, PowerShell, Command Prompt, and WSL can be selected intentionally on Windows.
- Cross-platform launch behavior is easier to reason about because it is centralized in one resolver.

Trade-offs:

- Provider startup now depends on shell quoting and host-shell compatibility, especially on Windows.
- Shell discovery on Windows remains heuristic rather than API-driven.
- Provider launch testing must cover both direct executables and shim-based CLIs.

Operational guidance:

- Treat shell selection as a runtime host setting, not just a workflow-command preference.
- When debugging launch failures, inspect both the provider executable resolver and the shell wrapper chosen by the resolver.
