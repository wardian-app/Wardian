# Codex terminal scrollback on Windows: ConPTY implementation

- **Status:** Implemented and validated end-to-end. Codex scrollback works in the Wardian app via the bundled modern ConPTY; the OSC color-probe regression is fixed. Packaging (vendored binaries + `build.rs` + `tauri.conf.json` + `load_conpty()` patch) is wired for Windows x64.
- **Date:** 2026-06-15
- **Area:** `src-tauri` PTY layer (`vendor/portable-pty`), Windows ConPTY; frontend terminal rendering (`src/features/terminal`).

## Problem

On Windows, codex (codex-rs / ratatui) rendered in Wardian's terminal was **not scrollable**: after producing a long response, the earlier output could not be scrolled back to. In a real terminal (Windows Terminal) the same codex session **is** scrollable — the conversation banner and prior output scroll up into the terminal's **native scrollback**, exactly like any other program's output. (This is in contrast to opencode/OpenTUI, which render their own in-TUI scrollbar and own their scroll.)

## What it is NOT (ruled out by live experiment)

Several plausible causes were tested against codex's real byte stream (captured under a raw ConPTY via a throwaway `portable-pty` harness) and **ruled out** — none changed codex's output (it stayed home-anchored repaints with zero scroll regions, xterm `baseY=0` at matched size):

- **Answering the DSR cursor probe.** Codex emits `ESC[6n` twice at startup and *hard-blocks* on the reply (no answer → it renders nothing at all). But answering it only unblocks rendering; it does not change the scroll behaviour.
- **DSR cursor position value** (tried bottom row vs default).
- **`WT_SESSION` / Windows-Terminal detection.** Codex's `terminal-detection` crate only drives palette, notifications, keymaps, and image protocols — not scroll/history. Setting `WT_SESSION` changed nothing.
- **`PSEUDOCONSOLE_PASSTHROUGH_MODE` (0x8)** on the in-box ConPTY — silently ignored by kernel32's `CreatePseudoConsole` without a sideloaded modern ConPTY, so inconclusive (superseded by the real fix below).
- **Redundant PTY resizes** purging scrollback — `baseY` never actually dropped on resize in any capture (see the separate resize-dedup note below).
- **Codex-owned wheel scrolling** — codex enables no mouse mode, so it is not codex consuming wheel events.

## Root cause

codex commits finished history to the terminal via ratatui's `insert_history_lines`, which emits **DECSTBM scroll-region** sequences (`ESC[1;<top>r` + `\r\n` + content). On Windows this passes through **ConPTY**.

`vendor/portable-pty`'s `load_conpty()` prefers a **sideloaded `conpty.dll`** next to the executable, but otherwise falls back to the **OS kernel32 `CreatePseudoConsole`**. That in-box kernel32 ConPTY **flattens codex's scroll-region history into home-anchored `ESC[H` full-window repaints** and does **not** forward scrolled-off lines to the downstream terminal's scrollback. Windows Terminal does not hit this because it **bundles its own modern ConPTY** (`OpenConsole.exe` + proxy) that forwards history correctly.

So: same codex, same protocol — the difference is purely the **ConPTY implementation** sitting between codex and the terminal.

## Evidence

Live capture of codex running "Output 100 lines of numbers", fed into a headless xterm:

| ConPTY | scroll-region seqs | `ESC[H` repaints | `baseY` @120×40 (matched size) |
|---|---|---|---|
| in-box kernel32 | 0 | 106 | **0** (not scrollable) |
| modern redistributable | **206** | 0 | **104** (full, clean history) |

With the modern ConPTY the headless buffer contains the codex banner at the top of scrollback and all 100 numbers in order — i.e. real native scrollback identical to Windows Terminal.

End-to-end in the actual Wardian app (codex real-provider rendering audit), modern `conpty.dll` placed next to `Wardian.exe`:

| state | `baseY` before (kernel32) | `baseY` after (modern ConPTY) |
|---|---|---|
| settled | 0 | 41 |
| wide | 0 | 124 |
| card-restored | 34 | 205 |
| maximized | — | 334 |
| rapid-resize-final | — | 581 |

Scrollback grows steadily, survives resizes, and the wheel scrolls it (`no_scrollback=0` throughout).

## Fix

Bundle the Microsoft ConPTY redistributable (`conpty.dll` + `OpenConsole.exe`, the same ConPTY Windows Terminal uses) so it lands **next to Wardian's executable**; `load_conpty()` then prefers it automatically. **No change to the PTY code path is required.**

Source: the `Microsoft.Windows.Console.ConPTY` NuGet package (x64 `conpty.dll` SHA-256 `C46DCD04F52B97F6A8CF53E8F547C85A821660BED18DE2B3344AFCD4A8389AD6`, `OpenConsole.exe` SHA-256 `47828C3FE080212F69DFDB39AB3673170FCC7445924C76FE003CEFD18247DD5D`). The OpenConsole/Windows Terminal project is MIT-licensed; redistribution with attribution is permitted.

### Rollout plan (decided)

- **Scope:** Windows-only. macOS/Linux use Unix PTYs in `portable-pty`; `load_conpty()` and the ConPTY path don't exist there, so non-Windows bundles are untouched.
- **Architecture:** **x64 only.** The release matrix (`.github/workflows/release.yml`) builds a single `windows-latest` target with no `--target` (default `x86_64-pc-windows-msvc`); the only `aarch64` targets are macOS. No Windows-on-ARM release, so no arm64 ConPTY needed.
- **Vendoring:** **commit** `conpty.dll` + `OpenConsole.exe` under `src-tauri/conpty/x64/` with a README pinning version (`Microsoft.Windows.Console.ConPTY` 1.25.x), SHA-256 (above), and MIT license. Reproducible, offline, no CI network dependency — mirrors node-pty's `third_party/conpty/<version>/`.
- **Placement — two surfaces** (binaries must sit *next to* the exe: `load_conpty()` does `LoadLibrary("conpty.dll")` and `conpty.dll` spawns `OpenConsole.exe` from the same dir):
  1. **Dev / direct-run** (`target/debug`, `target/release`): extend the existing `#[cfg(windows)]` block in `src-tauri/build.rs` to copy both binaries next to the built exe on every Windows build.
  2. **Installer (NSIS):** declare them in `tauri.conf.json` `bundle.resources` using the **map form** (`"conpty/x64/conpty.dll": "conpty.dll"`, etc.) so they land at the install root beside the exe (not a `resources/` subdir). Verify on a built installer.
- **Fallback:** automatic — if the binaries are absent, `load_conpty()` falls back to kernel32 (today's behaviour), so this cannot break startup.
- **Optional (later):** a `windowsUseConptyDll`-style setting (default on), mirroring VS Code. Not required for v1.

### Prior art: VS Code / node-pty (confirmed)

VS Code bundles its own ConPTY and prefers it over the in-box kernel32 one — this is the de-facto standard, and we should mirror it:

- **node-pty** (`src/win/conpty.cc`, `LoadConptyDll()`) loads a sideloaded `conpty/conpty.dll` relative to the native module when its `useConptyDll` flag is set, otherwise falls back to `kernel32.dll` `CreatePseudoConsole`. `scripts/post-install.js` copies the binaries from `third_party/conpty/<version>/win10-{x64,arm64}/` into `build/Release/conpty/` (`conpty.dll` + `OpenConsole.exe`).
- **VS Code** exposes `terminal.integrated.windowsUseConptyDll` (**defaults to true**) and ships the redistributable via node-pty; it even drops kill/spawn throttling when the bundled ConPTY is active because the newer ConPTY fixed those hangs. The version VS Code currently bundles is `1.25.260303002`.
- **License:** MIT (`Microsoft.Windows.Console.ConPTY` nuspec declares MIT, `requireLicenseAcceptance="false"`) — redistribution in a third-party app is explicitly permitted.

**Implication for Wardian:** `portable-pty`'s `load_conpty()` already implements exactly node-pty's prefer-sideloaded-else-kernel32 fallback, so simply placing `conpty.dll` + `OpenConsole.exe` next to the executable mirrors VS Code with **zero PTY-code change**, and the kernel32 fallback preserves current behaviour when the binaries are absent. Optionally surface a setting (default on) like VS Code's `windowsUseConptyDll` for an escape hatch.

## Follow-up regression: OSC color-probe garbage (fixed)

Switching codex to the modern ConPTY surfaced a regression: codex's composer was spammed at startup with stray `]10;rgb:…` / `]11;rgb:…` text. Cause: under the modern ConPTY codex now emits OSC 10/11 (and `ESC[?996n`) **color probes** (it did not under kernel32), and **two** responders answered — Wardian's frontend (`planTerminalCapabilityResponses`, 2-digit `rgb:eb/eb/eb`) and **xterm.js's own built-in auto-reply** (4-digit `rgb:ebeb/…`). Codex consumed one and echoed the duplicate into its composer.

Fix (`terminalCapabilities.ts`), validated by the audit (garbage lines 644 → **0**, scrollback unchanged):
1. `respondsToThemeColorQueries()` (opencode/antigravity only, **codex excluded**) now gates the OSC 10/11/palette + light-dark replies — codex no longer gets Wardian's frontend reply. `supportsTerminalThemeResponses()` still includes codex so its composer-background **output** normalization continues.
2. `stripTerminalColorQueries()` removes codex's OSC 10/11/4 + `ESC[?996n` probes from its output **before** it reaches xterm.js, suppressing xterm's auto-reply. (Codex does not block on these probes, and OpenConsole answers them anyway.)

## Related findings (separate changes)

- **Resize dedup (perf):** `manager::resize_pty` now drops resizes whose dimensions exactly match the last recorded PTY size, and the initial size is seeded at spawn (`spawn.rs`). ConPTY's `ResizePseudoConsole` is unconditional and an identical-size resize still makes inline TUIs do a full redraw; deduping avoids that churn. This is a correctness/perf improvement, **not** the scrollback fix (kept as its own commit).
- **DSR startup block:** codex blocks on its startup `ESC[6n` probe. With the modern ConPTY, OpenConsole answers it and codex renders fully, so no Wardian-side DSR responder is needed. (A frontend DSR answer was prototyped and reverted — it arrived too late, after the initial PTY backfill, to matter.)
- The comment in `src/features/terminal/terminalCapabilities.ts` describing these TUIs as having "no recoverable history" is **incorrect for a real terminal** and should be revised once the fix lands.

## Implementation (landed)

- `src-tauri/conpty/x64/{conpty.dll,OpenConsole.exe}` — vendored binaries + `src-tauri/conpty/README.md` (version 1.24.260512001, SHA-256, MIT, and a note on what to add for a future arm64 release).
- `vendor/portable-pty/src/win/psuedocon.rs` `load_conpty()` — prefer `<exe_dir>/conpty/x64/conpty.dll` (absolute path, so it finds its co-located `OpenConsole.exe`), then a bare sideloaded `conpty.dll`, then kernel32.
- `src-tauri/build.rs` — Windows-only best-effort copy of `conpty/x64/*` next to the built exe for dev/direct-run.
- `src-tauri/tauri.conf.json` `bundle.resources` — `conpty/x64/conpty.dll` + `OpenConsole.exe` for the installer (lands at `<install>/conpty/x64/`).
- `src/features/terminal/terminalCapabilities.ts` — OSC color-probe regression fix (`respondsToThemeColorQueries` + `stripTerminalColorQueries`).

Validated end-to-end via the codex real-provider rendering audit on the rebuilt debug app (no manual binary placement): scrollback `baseY` 0 → 423 and climbing across resizes, OSC garbage lines 0.

The throwaway investigation scaffolding (`vendor/portable-pty/examples/codex_capture.rs`, the temporary `[[example]]`/`[workspace]` additions to that crate's `Cargo.toml`, `scripts/_measure_baseY.cjs`, and stray test copies of the binaries) has been removed.
