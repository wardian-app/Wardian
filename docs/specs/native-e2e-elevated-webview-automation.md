# Native E2E Automation on Elevated Windows Runners

## Decision

Wardian's Windows native-E2E job configures the machine-level WebView2
`AdditionalBrowserArguments` policy for `Wardian.exe` with
`--remote-debugging-port=0` before starting the focused native suite.

This policy is limited to the ephemeral CI worker. Wardian's production build
does not ship with remote debugging enabled.

## Context

WebView2 Runtime 150 added security hardening for elevated host processes. It
ignores browser arguments supplied through user-writable channels, including
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. GitHub-hosted Windows jobs run at high
integrity, while `tauri-driver` depends on that argument to enable its dynamic
DevTools port. The resulting session fails before an application assertion
with `DevToolsActivePort file doesn't exist`.

Microsoft documents machine-level policy or application API configuration as
the supported channels for elevated hosts. Wardian uses machine policy in CI
because the automation switch belongs to the test environment, not the
shipping application. The policy value is scoped to the Wardian executable
rather than every WebView2 application on the worker.

Upstream context:
[WebView2Feedback #5640](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5640).

## Verification

The Windows native-workbench job must reach and pass its focused native suite
on WebView2 Runtime 150 or newer. A successful build alone is insufficient;
the suite proves that `tauri-driver` attached to the native WebView and ran the
workbench lifecycle assertions.
