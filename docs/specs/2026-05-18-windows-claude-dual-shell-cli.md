# Windows Claude Dual-Shell CLI Access

## Status

Implemented.

## Context

GitHub issue #272 reported that Windows Claude sessions could not reliably run the `wardian` CLI from shell tools. Wardian installed a `wardian.cmd` shim for native Windows shells, but Claude can also execute commands through Git Bash-compatible POSIX shells where `.cmd` dispatch is not the primary lookup surface.

## Decision

On Windows, Wardian installs two launchers in the managed CLI `bin` directory:

- `wardian.cmd` for PowerShell and cmd.
- `wardian` for POSIX shell lookup through Git Bash, MSYS2, and similar bash environments.

Wardian-managed provider processes also receive the active managed CLI `bin` directory at the front of `PATH`. The PATH injection is Windows-only and is applied to interactive PTY launches and headless provider processes.

Wardian also repairs legacy Wardian-owned launchers under `%USERPROFILE%\bin` when the default Wardian home is installed. Some Claude Code bash tool shells can prefer `~/bin` ahead of the inherited process `PATH`; if `~/bin/wardian` or `~/bin/wardian.cmd` is an older Wardian launcher, Wardian rewrites it as a forwarder to the active `%USERPROFILE%\.wardian\bin\wardian-cli.exe`. Non-Wardian user scripts are left untouched.

## Behavior

- PowerShell and cmd resolve `wardian` through `wardian.cmd`.
- Git Bash resolves `wardian` through the extensionless POSIX launcher.
- The POSIX launcher delegates to `wardian-cli.exe` beside the launcher.
- PATH injection is idempotent and does not duplicate the managed `bin` directory if it is already present.
- Legacy Wardian-owned `~/bin` launchers forward to the active managed CLI so Claude tool shells do not run stale versions when `~/bin` is shell-prepended.

## Verification

Local Windows smoke checks were run with a temporary managed `bin` directory containing `wardian-cli.exe`, `wardian.cmd`, and the extensionless POSIX launcher:

- PowerShell resolved `wardian` to `wardian.cmd` and `wardian --version` returned `wardian 0.3.5`.
- Git Bash resolved `wardian` to the extensionless launcher and `wardian --version` returned `wardian 0.3.5`.

The runtime-level smoke remains to launch a Wardian-managed Claude session on Windows and run `wardian agent` from both native shell and bash-backed tool contexts inside that session.
