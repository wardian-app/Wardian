# Runtime resource profiling

Wardian resource investigations separate three process groups:

- the Rust `Wardian` backend;
- WebView2 processes that render the application UI;
- provider runtimes and helpers supervised by Wardian.

Do not combine those groups into one memory or CPU number. Provider runtimes
often dominate total memory, while backend polling or indexing can still make
Wardian itself unnecessarily expensive.

## Build the application under test

A production-profile Tauri binary must include the custom protocol feature.
Building the Rust package without that feature produces an executable whose
WebView navigates to a development `localhost` URL.

```sh
npm run build
cd src-tauri
cargo build -p Wardian --release --features tauri/custom-protocol
```

PowerShell (Windows):

```powershell
npm run build
Set-Location src-tauri
cargo build -p Wardian --release --features tauri/custom-protocol
```

## Enable source-level counters

Source-level profiling is opt-in. It writes aggregate counters to
`<wardian-home>/wardian_debug.log`; it does not record agent identifiers,
paths, terminal content, or provider payloads.

```sh
WARDIAN_RUNTIME_PROFILE=1 \
WARDIAN_RUNTIME_PROFILE_INTERVAL_SECONDS=10 \
  ./target/release/Wardian
```

PowerShell (Windows):

```powershell
$env:WARDIAN_RUNTIME_PROFILE = '1'
$env:WARDIAN_RUNTIME_PROFILE_INTERVAL_SECONDS = '10'
& '<workspace-path>\target\release\Wardian.exe'
```

Use at least six intervals after restoration settles. Timings for nested
boundaries are not additive. For example, provider reconciliation is part of a
complete metrics tick.

## Capture the Windows process split

The checked-in PowerShell profiler samples CPU, private and working-set memory,
thread and handle counts, and process I/O. It intentionally omits command
lines, environment values, agent names, and paths.

```powershell
./scripts/profile-wardian-runtime.ps1 `
  -DurationSeconds 60 `
  -WardianProcessId <pid> `
  -OutputPath <profile.json>
```

Compare runs only when their known-agent, live-provider, and ConPTY counts are
similar. CPU is reported as average cores, so `1.0` means one logical processor
was continuously busy during the sample.

## Inspect WebView memory

The WebView profiler requires a diagnostic relaunch with a local DevTools port.
Do not expose that port beyond loopback, and return to a normal launch after the
capture.

PowerShell (Windows):

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
& '<workspace-path>\target\release\Wardian.exe'
node scripts/profile-wardian-webview.mjs http://127.0.0.1:9222 --sample-seconds=10
```

`--collect-garbage` is diagnostic: it helps distinguish retained live memory
from reclaimable heap, but the resulting footprint is not a normal-runtime
measurement.

## Safety and interpretation

Close Wardian through its normal window lifecycle before replacing a running
binary. Do not attach a debugger merely to obtain attribution from a live user
session; a debugger can suspend or terminate the process and its supervised
workloads.

Treat high process-I/O counters as call attribution evidence, not automatically
as physical disk traffic. On Windows, cached reads and IPC also contribute.
Pair process measurements with source-level counters before choosing an
optimization target.
