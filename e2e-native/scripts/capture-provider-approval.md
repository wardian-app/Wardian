# Coordinator approval-profile discovery

This script belongs to #1170. It is separate from the accepted seven-file frozen
patch. Paid captures remain serialized by the coordinator. The current harness
allocates private ports and verifies listener ownership. The sidecar has not
executed it. The coordinator owns execution and provider quota decisions.

Claude v7 Haiku reported a session limit until **2026-09-07 20:10 UTC**
(16:10 local in the coordinator's report). The script blocks Claude before that
instant. Do not change authentication, billing, or the Claude model to evade
the limit. Codex is a separate explicit coordinator selection, never an
automatic response to a quota failure.

The script submits at most one non-destructive scratch-file write. It captures
the terminal, current status, screenshot, transcript, and source/session fields
without sending approval keys. It then pauses only its newly created agent and
requires acknowledged pause, Off status, and an absent scratch file before and
after settlement and app closure. Pausing terminates the pending runtime; this
is not evidence that a provider rejection choice works.

## Execute one capture

1. Finish the current coordinator-owned native run. Select an already-built
   isolated artifact. Record the installed Claude version from the coordinator's
   preflight. Wait until the Claude reset above if selecting Claude. Do not reuse
   another run's Wardian home.
2. After the reset, run the script once from the repository root with Claude
   manual/Haiku:

   ```sh
   WARDIAN_E2E_CAPTURE_APPROVAL=1 \
   WARDIAN_E2E_CAPTURE_PROVIDER=claude \
   WARDIAN_E2E_CAPTURE_PROVIDER_VERSION='<observed-installed-version>' \
   WARDIAN_NATIVE_APP='<absolute-prebuilt-artifact-path>' \
   node e2e-native/scripts/capture-provider-approval.mjs --coordinator-serial
   ```

   PowerShell:

   ```powershell
   $env:WARDIAN_E2E_CAPTURE_APPROVAL = '1'
   $env:WARDIAN_E2E_CAPTURE_PROVIDER = 'claude'
   $env:WARDIAN_E2E_CAPTURE_PROVIDER_VERSION = '<observed-installed-version>'
   $env:WARDIAN_NATIVE_APP = '<absolute-prebuilt-artifact-path>'
   node e2e-native/scripts/capture-provider-approval.mjs --coordinator-serial
   ```

3. Inspect `approval-capture.json` in the printed private capture home. A complete
   capture requires `status=pending_candidate_captured`, `success=true`,
   `submissions=1`, `approval_keys_sent=0`, and
   `cancellation.status=confirmed_paused`. Require all recorded absence checks
   to be true. `capture_failed` or `cancellation.status=unconfirmed` is not a
   complete capture. Never replay an uncertain submission.
4. Compare `before-submit.json/png`, `action-required.json/png`, and
   `pending-transcript.json`. Identify the tool-specific prompt and both current
   choices from those observations. A startup trust/model menu, echoed request,
   or quota error does not establish a tool-approval profile. Retain missing
   native-call projection or missing provider identity as explicit gaps.
5. Fill a separate local profile only from the observed evidence. The accepted
   suite requires `provider`, `provider_version`, `evidence`, `ready_text`,
   `prompt_text`, `deny_choice`, `allow_choice`, `deny_keys`, and `allow_keys`.
   Capture leaves all choice/key fields null. Do not assume an arrow count,
   default selection, shortcut, or Return behavior from another provider/version.
   Keys require displayed navigation instructions or separately authorized
   evidence; this capture does not exercise them.

For an explicitly chosen Codex capture, first establish the native protocol and
choices through the coordinator's preflight. Set
`WARDIAN_E2E_CAPTURE_CODEX_PREFLIGHT` to the absolute retained evidence file. The
script requires a nonempty file and records its hash; it does not interpret that
file as proof that a choice was tested. The coordinator owns that assessment.
Select Codex by
setting `WARDIAN_E2E_CAPTURE_PROVIDER=codex`,
`WARDIAN_E2E_CAPTURE_PROVIDER_VERSION`, and
`WARDIAN_E2E_CAPTURE_MODEL=gpt-5.4-mini`. Codex uses
low effort and untrusted/read-only settings. On Windows, the single scratch
operation requests `exec_command` with `shell=cmd.exe` and `login=false`; its
command writes only the newly generated scratch path. There is no automatic fallback or startup-menu
selection. A required startup choice produces retained evidence and stops before
submission.

All screenshots, terminal text, prompts, local paths and session identifiers
remain private in the retained test home. Sanitize evidence before publication.
No product files, provider authentication, host sessions, or accepted frozen
suite files are changed. There is no build, automatic review dispatch, New
Session action, or matrix update in this script.
