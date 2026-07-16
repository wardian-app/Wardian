# Native E2E WebView Driver Selection

## Decision

Wardian's Windows native E2E setup must provision and select a Microsoft Edge
WebDriver matched to the installed WebView2 runtime. It must not implicitly
reuse a standalone Edge or Chrome driver found on the machine's `PATH`.

An explicit `WARDIAN_NATIVE_WEBDRIVER` remains the supported override. Without
that override, setup downloads the matching driver into
`tools/e2e-native/`, and the native harness selects that local driver first.

## Rationale

Tauri automates the embedded WebView2 runtime rather than the standalone Edge
browser. Hosted CI images can update those components on different patch
schedules. A browser-matched runner driver may therefore launch the Wardian
process but fail to attach to its WebView with a `DevToolsActivePort file
doesn't exist` session error.

Keeping the selected driver local to Wardian makes native verification
deterministic across runner-image rollovers while preserving an explicit path
for custom or offline driver installations.

## Verification

- The setup-script test asserts that Windows reuse candidates are restricted
  to Wardian's local native-tools directory.
- The Windows native-workbench CI job exercises the provisioned driver against
  the built Tauri application.
