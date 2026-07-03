# Claude Windows ConPTY Diagnostics

- **Status:** Implemented as a diagnostics and cleanup slice.
- **Date:** 2026-07-02
- **Area:** Windows PTY runtime, bundled ConPTY/OpenConsole packaging, frontend terminal rendering.
- **Issue:** [#623](https://github.com/wardian-app/Wardian/issues/623)

## Context

A Windows Claude Code user reported duplicate terminal lines even after Wardian's modern ConPTY/OpenConsole integration. The issue occurred more often before the OpenConsole bundle landed, so the highest-value question is whether that user's installed Wardian build is actually loading the bundled ConPTY implementation or silently falling back to another backend.

## Findings

- Wardian's direct dev/release build path copies `conpty/x64/conpty.dll` and `OpenConsole.exe` beside `Wardian.exe`.
- Existing NSIS artifacts inspected locally contain `conpty/x64/conpty.dll` and `conpty/x64/OpenConsole.exe`, so the current installer shape can package the files correctly.
- The loader still falls back by design: bundled ConPTY, then bare `conpty.dll`, then kernel32. That fallback should remain; the missing surface was runtime proof of which path won.
- User settings should not expose a switch to disable modern ConPTY. The modern backend is the intended default, and fallback exists for missing or failing bundles.
- Claude settings and custom CLI options can affect Claude behavior, but they do not select Wardian's frontend terminal implementation or ConPTY backend.
- The active xterm debug snapshot already exposes `bufferType`, which is the useful frontend signal for normal versus alternate screen buffer state.
- The old synthetic viewport-redraw path is unreachable because provider redraw routing is disabled. It is a cleanup candidate, but it is not part of this diagnostics slice because this PR should stay focused on proving the Windows ConPTY runtime path.

## Implementation

Wardian now exposes terminal runtime diagnostics with:

- platform;
- Windows ConPTY load source: `bundled`, `bare`, or `kernel32`;
- expected bundled `conpty.dll` path and existence;
- expected bundled `OpenConsole.exe` path and existence;
- load errors for bundled or bare fallback attempts when present.

The same diagnostics are logged once when an agent PTY or user-terminal PTY starts, so an affected user can provide evidence from their Wardian debug log without guessing whether bundling failed.

## Investigation Use

For a Windows duplicate-line report, collect:

1. The terminal runtime diagnostics entry from the Wardian debug log.
2. The frontend terminal debug snapshot for the affected session, especially renderer `bufferType`, `baseY`, `viewportY`, and recent write previews.
3. Whether the duplicate lines appear after resize, maximize/minimize, font/layout changes, or normal output.

If `load_source` is not `bundled`, investigate installer artifact, install directory contents, architecture, or blocked file loading before changing terminal rendering behavior.

If `load_source` is `bundled`, treat the next hypothesis as Claude normal-buffer resize/reflow behavior and require a raw terminal trace before adding any scrollback deduplication, because previous dedup attempts risked deleting legitimate terminal history.

Separately, the unreachable synthetic viewport-redraw path can be deleted in a frontend cleanup PR with its own screenshot-policy handling.
