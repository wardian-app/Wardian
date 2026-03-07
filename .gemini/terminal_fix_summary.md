# Terminal Rendering & Stability Fix Summary

This document summarizes the persistent terminal rendering issues, the architectural root causes, and the multi-layered fixes that eventually stabilized the terminal environment in Wardian.

---

## 🔴 Identified Bugs & Regressions

1.  **Duplicate Boxes at Startup**: The subagent delegation TUI (priority boxes) would often appear twice or corrupted when a new agent was spawned.
2.  **Blank Terminals at Startup**: Terminals occasionally appeared empty until the first manual resize.
3.  **"Reflow Hell" / One-Column Distortion**: Terminals initializing with 1x1 or 1-column widths, causing xterm.js to reflow data into thousands of rows, leading to high CPU usage (up to 100%) and scrambled vertical text.
4.  **Resize History Corruption**: When transitioning from **Maximized** view back to **Grid** view, the scrollback history often showed duplicated animation frames or "ghost" lines of TUI elements.
5.  **Zombie `conhost.exe` Processes**: Orphaned console host processes accumulating on Windows after sessions were supposedly closed.

---

## 🔍 Root Cause Analysis

The core issue was a fundamental conflict between **xterm.js's internal reflow mechanism** and **Windows ConPTY's redraw behavior**.

- **xterm.js Reflow**: When the terminal container shrinks, xterm.js automatically wraps lines to fit the new width. This shifts the internal buffer lines upward.
- **ConPTY Redraw**: When the backend PTY is resized, Windows ConPTY forces a full redraw of the current visible screen.
- **The Conflict**: If xterm.js reflows _before_ ConPTY's redraw arrives, the new redraw from ConPTY is printed on top of the already-shifted wrapped lines, resulting in visible duplication in the scrollback.
- **Race Condition**: During startup, `fitAddon.fit()` and `term.onResize()` were often triggering backend resizes simultaneously, causing ConPTY to redraw the TUI multiple times.

---

## 🛠️ Failed or Partial Attempts

- **`windowsMode: true`**: Deprecated in modern xterm.js versions and did not address the ConPTY overlap.
- **`term.clear()` on Resize**: While it prevented duplication, it was too destructive as it wiped the entire scrollback history.
- **ANSI Viewport Clearing (`\x1b[2J`)**: Attempted to clear only the visible screen before ConPTY redrew, but it still caused visual flickers and didn't perfectly align with the PTY state.
- **Status Gating**: Attempted to delay rendering until the agent was "Processing", but this caused blank terminals for idle/delegating agents.

---

## ✅ Final Successful Fixes (The "Multi-Layered Defense")

The solution involved a coordinated effort between the frontend and backend to strictly enforce minimum dimensions and timing:

### 1. Unicode & Dimension Precision

- **Unicode11Addon**: Added to xterm.js. This ensures that box-drawing characters and wide characters are measured with absolute precision, preventing the "off-by-one" column wrapping that triggers unnecessary ConPTY reflows.

### 2. Frontend: Consolidated Gated Initialization (`App.tsx`)

- **`performFit` Helper**: All fitting logic was consolidated into a single `useCallback` that checks `el.offsetParent !== null` (visibility) and enforces a strict `term.cols > 10` gate.
- **Debounced PTY Resizing**: Implemented a **50ms debounce** on the `invoke("resize_agent_terminal")` call within `onResize`. This allows the browser's layout engine and xterm.js's internal reflow to stabilize _before_ telling the Windows backend to redraw the screen.
- **Initial Sizing Guard**: Replaced immediate `fit()` calls with a `checkSizingAndStart` routine. This uses `requestAnimationFrame` and staggered `setTimeout` checks to ensure the terminal only starts its polling loop once a stable layout is achieved.

### 3. Backend: Safety Resize Gate (`manager.rs`)

- **PtySize Guard**: Added a safety check at the start of `resize_pty` to ignore any requests where `cols < 10`. This acts as a circuit breaker, preventing the PTY from ever entering a high-CPU reflow state even if the frontend sends unstable dimensions.
- **Synchronous Spawn Blocking**: Maintained the `spawn_blocking` resize with a watchdog timer to prevent ConPTY deadlocks without blocking the entire tokio pool.

### 4. Deterministic Process Reaping

- **Job Objects (Windows)**: Confirmed that child processes are assigned to Windows Job Objects for reliable "Kill Tree" behavior.
- **Explicit Job Drop**: We now use `job_object.take().unwrap()` during session removal to ensure the Windows Job Object is explicitly dropped, cascading the kill signal to the entire ConPTY/Conhost process tree and preventing zombies.

### 5. Viewport Thrashing & TUI Word-Wrap Defense

- **Node Native SIGWINCH Bypass**: Wardian spawns the CLI by resolving the absolute path to `gemini-cli/dist/index.js` and executing it directly with `node.exe`. This bypasses the default `gemini.cmd` batch script on Windows, which previously swallowed `SIGWINCH` resize events. Consequently, the TUI React Ink engine now receives accurate `resize` events natively from ConPTY, preventing the engine from rendering massive, stale-height views that cause "Viewport Thrashing" against the scrollback.
- **TUI Line Wrap Prevention (The 650px Rule)**: The `gemini-cli` hardcodes specific box-drawing UI elements. If the terminal width (`cols`) shrinks below the width of these strings, Windows ConPTY natively word-wraps the output. However, the internal TUI rendering engine remains unaware of the wrap, causing it to miscalculate its own height by 1 line. Since it redraws multiple times per second, this single mathematical desync pushes the entire scrollback history downwards in an infinite loop.
- **Strict Minimum Layout Metrics**: To mathematically eradicate the horizontal bug, the application requires exactly `1325px` of center stage real-estate on a 1080p display (achieved by shrinking sidebars). This allows both outer flex cards and inner `xterm.js` containers to strictly enforce `min-w-[650px]`, guaranteeing the PTY width never drops below the CLI's hardcoded word-wrap limit. **Note**: These strict 650px bounds can be relaxed in the future if/when the `gemini-cli` ceases to utilize hardcoded fixed-width UI boxes.
- **Scrollback Optimization**: Reduced terminal scrollback from 5000 to 1000 lines. This aggressively reduces the amount of text xterm.js must reflow during a resize operation, significantly improving UI responsiveness when toggling between Grid and Maximized views.

---

## 🏗️ Core Architecture (The Foundation)

All these fixes rely on the **Pull-Based Data Model** (drain-on-read):

- The frontend **pulls** data using `requestAnimationFrame` and `read_agent_pty`.
- This provides natural backpressure; the backend only sends data as fast as the terminal can render it, preventing IPC bridge saturation and the "lag-induced re-stuttering" seen in the previous implementation.
