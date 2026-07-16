# OpenCode Terminal Protocol Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the desktop and remote OpenCode terminal experience by letting xterm and OpenTUI retain negotiated mouse, alternate-screen, selection, synchronized-output, and geometry ownership.

**Architecture:** `terminalCapabilities.ts` remains the narrow transport compatibility boundary and stops deleting valid OpenTUI protocol. Desktop and remote interaction decisions use xterm's active buffer type and `modes.mouseTrackingMode`, never a provider-name proxy. Remote touch input adapts touch travel into bubbling wheel events only for alternate-screen mouse sessions; normal-buffer terminals retain Wardian viewport scrolling.

**Tech Stack:** React 19, TypeScript 5.8, xterm.js 6, Vitest, Testing Library, Tauri 2 native E2E, Selenium WebDriver.

## Global Constraints

- Preserve complete OpenCode SGR mouse reports, mouse DECSET/DECRST toggles, and synchronized-output controls.
- Drop only malformed OpenCode legacy no-button motion with decoded button code `35`; preserve legacy drag codes `32` through `34`.
- Report DECRQM mode `2026` as supported and reset (`CSI ? 2026 ; 2 $ y`).
- Use `terminal.buffer.active.type` and `terminal.modes.mouseTrackingMode` for behavior decisions.
- Keep owner/mirror presentation geometry from `origin/main`: mirrors render canonical geometry and never resize the PTY.
- Do not change mouse behavior for providers other than OpenCode through provider-specific filtering.
- Provider-specific behavior claims require the opt-in real OpenCode native E2E.
- Frontend behavior changes require an HTTPS screenshot embedded in the PR body.

---

### Task 1: Preserve OpenTUI Protocol And Narrow The Windows Guard

**Files:**
- Modify: `src/features/terminal/terminalCapabilities.ts`
- Test: `src/features/terminal/terminalCapabilities.test.ts`

**Interfaces:**
- Consumes: `filterProviderTerminalInput(provider, data, options)` and `planTerminalCapabilityResponses(provider, data, context)`.
- Produces: output normalization that retains mouse/sync controls and input filtering that removes only legacy button code `35`.

- [ ] **Step 1: Replace the old capability expectations with failing protocol-preservation tests**

```ts
it("preserves OpenCode mouse and synchronized-output modes", () => {
  const data = "\u001b[?1000h\u001b[?1002h\u001b[?1003h\u001b[?1006h\u001b[?2026hready\u001b[?2026l";
  expect(normalizeOpenCodeOutput(data, "opencode")).toBe(data);
});

it("reports synchronized output as supported and reset", () => {
  const plan = planTerminalCapabilityResponses("opencode", "\u001b[?2026$p", baseContext);
  expect(plan.outgoingInputs).toContain("\u001b[?2026;2$y");
});

it("preserves complete SGR mouse and legacy drag reports", () => {
  const sgrDrag = "\u001b[<32;12;8M";
  const legacyDrag = "\u001b[M" + String.fromCharCode(64, 44, 40);
  expect(filterProviderTerminalInput("opencode", sgrDrag + legacyDrag)).toBe(sgrDrag + legacyDrag);
});

it("drops only legacy no-button motion", () => {
  const passive = "\u001b[M" + String.fromCharCode(67, 44, 40);
  expect(filterProviderTerminalInput("opencode", passive + "typed")).toBe("typed");
});
```

- [ ] **Step 2: Run the focused test and verify red**

Run: `npm run test -- src/features/terminal/terminalCapabilities.test.ts`

Expected: failures show mouse/sync toggles are stripped, mode `2026` is unsupported, and drag packets are removed.

- [ ] **Step 3: Implement the narrow protocol boundary**

```ts
const SUPPORTED_RESET_DECRQM_PARAMS = new Set([1004, 1016, 2004, 2026]);
const UNSUPPORTED_RESET_DECRQM_PARAMS = new Set([2027, 2031]);

function isLegacyPassiveMouseMotionButtonCode(buttonCode: number) {
  return Number.isFinite(buttonCode) && buttonCode === 35;
}

export function normalizeOpenCodeOutput(data: string, provider?: string) {
  if (!data) return data;
  if (provider !== "opencode") {
    return stripProviderScrollbackErase(normalizeFullscreenClearByNewlines(data), provider);
  }
  return stripProviderScrollbackErase(data, provider)
    .replace(DECRQM_QUERY, "")
    .replace(THEME_MODE_NOTIFICATION_TOGGLE, "");
}
```

Delete `SYNC_OUTPUT_TOGGLE`, `OPENCODE_MOUSE_TRACKING_TOGGLE`, and SGR mouse stripping. Use `isLegacyPassiveMouseMotionButtonCode` in both prefixed and binary legacy packet paths.

- [ ] **Step 4: Run the focused test and verify green**

Run: `npm run test -- src/features/terminal/terminalCapabilities.test.ts`

Expected: all terminal capability tests pass.

- [ ] **Step 5: Commit the protocol boundary**

```bash
git add src/features/terminal/terminalCapabilities.ts src/features/terminal/terminalCapabilities.test.ts
git commit -m "fix(terminal): preserve OpenCode protocol modes"
```

### Task 2: Make Desktop Wheel And Geometry Ownership State-Based

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Test: `src/features/terminal/AgentTerminal.test.tsx`

**Interfaces:**
- Consumes: xterm `Terminal.buffer.active.type` and `Terminal.modes.mouseTrackingMode`.
- Produces: `terminalOwnsMouseInteraction(term): boolean` and `shouldUseRenderedRowGeometry(term, force): boolean` exposed through `__terminalTesting`.

- [ ] **Step 1: Add failing alternate-screen ownership tests**

```ts
it("leaves alternate-screen mouse wheel input to xterm", async () => {
  render(<AgentTerminal sessionId="opencode-wheel" provider="opencode" theme="dark" />);
  await waitFor(() => expect(mockTerminal).toHaveBeenCalled());
  const instance = getLatestTerminalInstance();
  instance.buffer.active.type = "alternate";
  instance.modes.mouseTrackingMode = "any";
  instance.scrollLines.mockClear();
  fireEvent.wheel(screen.getByTestId("agent-terminal-host"), { deltaY: -240, deltaMode: 0 });
  expect(instance.scrollLines).not.toHaveBeenCalled();
});

it("chooses rendered row geometry by active buffer type", () => {
  const terminal = getLatestTerminalInstance();
  terminal.buffer.active.type = "normal";
  expect(__terminalTesting.shouldUseRenderedRowGeometry(terminal, false)).toBe(true);
  terminal.buffer.active.type = "alternate";
  expect(__terminalTesting.shouldUseRenderedRowGeometry(terminal, false)).toBe(false);
  expect(__terminalTesting.shouldUseRenderedRowGeometry(terminal, true)).toBe(false);
});
```

Update the xterm mock with `buffer.active.type = "normal"` and `modes.mouseTrackingMode = "none"`.

- [ ] **Step 2: Run the focused desktop test and verify red**

Run: `npm run test -- src/features/terminal/AgentTerminal.test.tsx`

Expected: OpenCode wheel still calls `scrollLines`, and the state-based geometry helper does not exist.

- [ ] **Step 3: Implement state-based ownership**

```ts
function terminalOwnsMouseInteraction(term: Terminal) {
  return term.buffer.active.type === "alternate" && term.modes.mouseTrackingMode !== "none";
}

function shouldUseRenderedRowGeometry(term: Terminal, force: boolean) {
  return !force && term.buffer.active.type === "normal";
}
```

At the start of `scrollTerminalFromWheel`, return `false` without preventing propagation when `terminalOwnsMouseInteraction(term)` is true. Replace `shouldUseRenderedRowGeometryForProvider(entry.provider, force)` with `shouldUseRenderedRowGeometry(renderer.term, force)`. Rename debug counter `opencode_owned` to `tui_owned`, remove the provider-only terminal CSS class, and export both helpers through `__terminalTesting`.

- [ ] **Step 4: Run the focused desktop test and verify green**

Run: `npm run test -- src/features/terminal/AgentTerminal.test.tsx`

Expected: all desktop terminal tests pass; the fabricated OpenCode normal-buffer test is gone.

- [ ] **Step 5: Commit desktop ownership**

```bash
git add src/features/terminal/AgentTerminal.tsx src/features/terminal/AgentTerminal.test.tsx
git commit -m "fix(terminal): honor alternate-screen mouse ownership"
```

### Task 3: Adapt Remote Wheel, Touch, Snapshot, And Geometry Behavior

**Files:**
- Modify: `src/features/remote/RemoteAgentDetailView.tsx`
- Test: `src/features/remote/RemoteMobileApp.test.tsx`

**Interfaces:**
- Consumes: the same xterm state contract from Task 2 and `RemoteTerminalSessionClient` owner/mirror state.
- Produces: `installTerminalScrollBridge` behavior where native wheel reaches xterm and touch dispatches a bubbling wheel event only for alternate-screen mouse sessions.

- [ ] **Step 1: Add failing remote ownership tests**

```ts
it("leaves wheel events to an alternate-screen mouse session", async () => {
  const terminal = vi.mocked(Terminal).mock.results.at(-1)?.value as Terminal;
  terminal.buffer.active.type = "alternate";
  terminal.modes.mouseTrackingMode = "any";
  fireEvent.wheel(screen.getByTestId("remote-terminal-scroll-surface"), { deltaY: -120 });
  expect(terminal.scrollLines).not.toHaveBeenCalled();
});

it("translates touch travel into a wheel event for alternate-screen mouse sessions", async () => {
  const terminal = vi.mocked(Terminal).mock.results.at(-1)?.value as Terminal;
  terminal.buffer.active.type = "alternate";
  terminal.modes.mouseTrackingMode = "any";
  const wheel = vi.fn();
  terminal.element.addEventListener("wheel", wheel);
  fireEvent.touchStart(terminal.element, { touches: [{ clientY: 220 }] });
  fireEvent.touchMove(terminal.element, { touches: [{ clientY: 184 }] });
  expect(wheel).toHaveBeenCalled();
  expect(terminal.scrollLines).not.toHaveBeenCalled();
});
```

Retain an explicit normal-buffer touch test that expects `scrollLines`.

- [ ] **Step 2: Run the focused remote test and verify red**

Run: `npm run test -- src/features/remote/RemoteMobileApp.test.tsx`

Expected: the capture-phase bridge prevents native wheel and maps alternate touch to `scrollLines`.

- [ ] **Step 3: Implement the touch adapter and state-based geometry**

```ts
function terminalOwnsMouseInteraction(terminal: Terminal) {
  return terminal.buffer.active.type === "alternate" && terminal.modes.mouseTrackingMode !== "none";
}

const onWheel = (event: WheelEvent) => {
  if (terminalOwnsMouseInteraction(terminal)) return;
  // existing normal-buffer row conversion and preventDefault
};

const onTouchMove = (event: TouchEvent) => {
  if (event.touches.length !== 1 || lastTouchY === null) return;
  const nextY = event.touches[0]?.clientY ?? lastTouchY;
  const deltaY = lastTouchY - nextY;
  if (terminalOwnsMouseInteraction(terminal)) {
    (terminal.element ?? measureHost).dispatchEvent(new WheelEvent("wheel", {
      bubbles: true,
      cancelable: true,
      deltaMode: WheelEvent.DOM_DELTA_PIXEL,
      deltaY,
    }));
  } else {
    touchRemainder = scrollByRows(touchRemainder + deltaY / terminalRowPixelHeight(terminal, measureHost));
  }
  lastTouchY = nextY;
  event.preventDefault();
};
```

In `proposedRemoteViewport`, use rendered-row correction only when `terminal.buffer.active.type === "normal"`; remove `agent.provider !== "opencode"`. Mirrors continue passing `useRenderedRowGeometry: false`. Ensure snapshot/live output uses the protocol-preserving normalization from Task 1 before stdin is enabled.

- [ ] **Step 4: Run the focused remote test and verify green**

Run: `npm run test -- src/features/remote/RemoteMobileApp.test.tsx`

Expected: remote terminal tests pass for alternate wheel, alternate touch, normal touch, owner sizing, and snapshot replay.

- [ ] **Step 5: Commit remote ownership**

```bash
git add src/features/remote/RemoteAgentDetailView.tsx src/features/remote/RemoteMobileApp.test.tsx
git commit -m "fix(remote): forward OpenCode mouse interactions"
```

### Task 4: Add Real OpenCode Proof And Update Developer Documentation

**Files:**
- Modify: `src/features/terminal/AgentTerminal.tsx`
- Modify: `e2e-native/tests/opencode-native.test.mjs`
- Modify: `e2e-native/tests/real-provider-rendering-native.test.mjs`
- Modify: `docs/developer/terminal-presentation-broker.md`

**Interfaces:**
- Consumes: `window.__wardianTerminalDebug.snapshot(presentationId)` and real xterm DOM events.
- Produces: opt-in assertions for alternate buffer, active mouse mode, unchanged draft during wheel scrolling, and OpenCode selection copy confirmation.

- [ ] **Step 1: Extend terminal debug snapshots with protocol state and assert it in native E2E**

Add `bufferType` and `mouseTrackingMode` to the desktop debug snapshot. In `opencode-native.test.mjs`, wait until:

```js
assert.equal(snapshot.renderer.bufferType, "alternate");
assert.notEqual(snapshot.renderer.mouseTrackingMode, "none");
```

Type a unique draft without submitting, dispatch wheel events at the `.xterm-screen`, and assert the draft remains visible and unchanged while other visible conversation rows change. Drag across a visible response row, send `Ctrl+C`, and wait for OpenCode's copied-selection confirmation text.

- [ ] **Step 2: Run the opt-in native test and verify behavior**

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_OPENCODE='1'
$env:WARDIAN_E2E_REAL_WORKSPACE='<absolute-workspace-path>'
npm run test:e2e:native:fast -- e2e-native/tests/opencode-native.test.mjs
```

Expected: PASS with a real OpenCode process. A missing provider/auth/runtime may skip only before the test begins; an interaction assertion must fail rather than skip.

- [ ] **Step 3: Remove the OpenCode scrollback exemption and document ownership**

Rename rendering-audit debug handling from `opencode_owned` to `tui_owned` and make alternate-screen OpenCode interaction evidence explicit instead of treating missing normal-buffer scrollback as success. Add a concise section to `terminal-presentation-broker.md` stating that alternate-screen mouse and synchronized-output modes are provider-owned while transport, lease ownership, and canonical geometry remain broker-owned.

- [ ] **Step 4: Capture feature-specific evidence**

Run the real-provider native test with screenshots written under `e2e/screenshots/opencode-terminal-protocol/<timestamp>/`. Keep a representative image showing scrolled conversation history with the unique draft unchanged or the copied-selection confirmation. Upload it to a GitHub-hosted HTTPS URL and embed it in PR #660.

- [ ] **Step 5: Commit E2E and docs**

```bash
git add e2e-native/tests/opencode-native.test.mjs e2e-native/tests/real-provider-rendering-native.test.mjs docs/developer/terminal-presentation-broker.md
git commit -m "test(opencode): verify native terminal interactions"
```

### Task 5: Full Verification And PR Update

**Files:**
- Verify all files changed since `origin/main`
- Update: PR #660 body and title through GitHub CLI

**Interfaces:**
- Consumes: completed Tasks 1 through 4.
- Produces: a force-with-lease updated PR linked to issues #659 and #665 with green checks and screenshot evidence.

- [ ] **Step 1: Run frontend and documentation verification**

```bash
npm run lint
npm run test
npm run build
npm run docs:check-llms
npm run docs:build
```

Expected: every command exits `0`.

- [ ] **Step 2: Run backend verification**

```bash
cd src-tauri
cargo clippy
cargo test
cargo check
```

Expected: every command exits `0`.

- [ ] **Step 3: Run native and screenshot gates**

Run the focused real OpenCode command from Task 4. Then set `PR_BODY` to the proposed PR body containing the uploaded HTTPS image and run `npm run check:frontend-screenshot`.

- [ ] **Step 4: Audit the diff and secrets**

```bash
git diff --check origin/main...HEAD
git status --short
git diff --name-only origin/main...HEAD
```

Inspect the diff for credentials, `.env` files, local absolute paths, generated binaries, and unrelated changes.

- [ ] **Step 5: Push the rebased branch and update PR #660**

```bash
git push --force-with-lease origin fix/terminal-row-fit-resize-loop
gh pr edit 660 --title "fix(terminal): restore regular OpenCode interactions" --body-file <pr-body-file>
gh pr checks 660 --watch
```

The PR body must use the repository template, explain the protocol-ownership root cause, include `Fixes #659` and `Fixes #665`, list exact verification commands, and embed the HTTPS screenshot.
