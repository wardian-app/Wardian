# Bundled ConPTY (Windows only)

Wardian ships the **modern Microsoft ConPTY** redistributable on Windows. The
in-box Windows (`kernel32`) ConPTY flattens an inline-viewport TUI's
scroll-region history (notably **codex**) into in-place repaints and does not
forward scrolled-off lines to the terminal's scrollback, so those agents render
as "not scrollable". The redistributable ConPTY — the same one Windows Terminal
and VS Code/node-pty use — forwards history correctly.

`vendor/portable-pty`'s `load_conpty()` (see `src/win/psuedocon.rs`) prefers
`conpty/x64/conpty.dll` next to the executable (loaded by absolute path so
`conpty.dll` finds its co-located `OpenConsole.exe`), then a bare sideloaded
`conpty.dll`, then falls back to `kernel32`. So if these binaries are absent the
behaviour is exactly the previous in-box ConPTY — they are an enhancement, not a
hard dependency.

See `docs/specs/2026-06-15-codex-conpty-scrollback.md` for the full root-cause
analysis and evidence.

## Contents

| File | SHA-256 |
| --- | --- |
| `x64/conpty.dll` | `C46DCD04F52B97F6A8CF53E8F547C85A821660BED18DE2B3344AFCD4A8389AD6` |
| `x64/OpenConsole.exe` | `47828C3FE080212F69DFDB39AB3673170FCC7445924C76FE003CEFD18247DD5D` |

## Source & version

- Package: [`Microsoft.Windows.Console.ConPTY`](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY)
- Version: **1.24.260512001**
- Upstream: [microsoft/terminal](https://github.com/microsoft/terminal) (OpenConsole)
- License: **MIT** (`requireLicenseAcceptance="false"`) — redistribution in a
  third-party application is permitted.

To update: download the package from nuget.org, replace the files under `x64/`
with `runtimes/win-x64/native/conpty.dll` and
`build/native/runtimes/x64/OpenConsole.exe`, and refresh the hashes/version
above.

## Architecture support

Currently **x64 only**, because Wardian's release pipeline
(`.github/workflows/release.yml`) builds a single Windows target
(`x86_64-pc-windows-msvc`); the only `aarch64` targets are macOS.

**If a Windows-on-ARM (arm64) release is ever added**, this also needs an
`arm64/` folder with the package's `runtimes/win-arm64/native/conpty.dll` and
`build/native/runtimes/arm64/OpenConsole.exe`, plus:
- `src-tauri/build.rs` — copy the arch-matched folder (select via
  `CARGO_CFG_TARGET_ARCH`) next to the built exe.
- `src-tauri/tauri.conf.json` `bundle.resources` — add the `conpty/arm64/*`
  entries.
- `vendor/portable-pty/src/win/psuedocon.rs` `load_conpty()` — pick the
  subfolder by target/host arch instead of the hard-coded `x64`.
