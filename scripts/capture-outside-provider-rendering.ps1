param(
  [Parameter(Mandatory = $true)]
  [ValidateSet("codex", "claude", "gemini", "opencode")]
  [string]$Provider,

  [string]$Workspace = (Get-Location).Path,
  [string]$OutputRoot = (Join-Path (Get-Location).Path "e2e\screenshots\outside-provider-rendering"),
  [int]$InitialWaitSeconds = 12,
  [int]$Columns = 0,
  [int]$Rows = 0,
  [int]$WindowWidth = 980,
  [int]$WindowHeight = 680,
  [int]$ResizedWindowWidth = 0,
  [int]$ResizedWindowHeight = 0,
  [int]$FontZoomSteps = 0,
  [string]$InputText = "",
  [string]$WardianHome = "",
  [string]$SessionId = "",
  [string]$ProviderSessionId = "",
  [string]$SessionName = ""
)

$ErrorActionPreference = "Stop"

$runId = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH-mm-ssZ")
$title = "WardianOutside-$Provider-$runId"
$windowName = "wardian-outside-$Provider-$runId"
$outDir = Join-Path $OutputRoot (Join-Path $runId $Provider)
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$sizeProbePath = Join-Path $outDir "terminal-size.json"
$ansiQueryPath = Join-Path $outDir "terminal-ansi-query.json"
$startSignalPath = Join-Path $outDir "start.signal"
$captureWindowWidth = $WindowWidth
$captureWindowHeight = $WindowHeight
$resizedCaptureWindowWidth = if ($ResizedWindowWidth -gt 0) { $ResizedWindowWidth } else { $captureWindowWidth }
$resizedCaptureWindowHeight = if ($ResizedWindowHeight -gt 0) { $ResizedWindowHeight } else { $captureWindowHeight }

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WardianWindowCapture {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@

function Wait-ProviderWindow {
  param([string]$Title, [int]$TimeoutSeconds = 30)
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $candidate = Get-Process WindowsTerminal -ErrorAction SilentlyContinue |
      Where-Object { $_.MainWindowTitle -like "$Title*" -and $_.MainWindowHandle -ne 0 } |
      Select-Object -First 1
    if ($candidate) {
      return $candidate
    }
    Start-Sleep -Milliseconds 250
  }
  throw "Timed out waiting for Windows Terminal window '$Title'"
}

function Save-WindowScreenshot {
  param([IntPtr]$Handle, [string]$Path, [int]$CaptureWidth = 0, [int]$CaptureHeight = 0)
  $HWND_TOPMOST = [IntPtr](-1)
  $HWND_NOTOPMOST = [IntPtr](-2)
  [WardianWindowCapture]::ShowWindow($Handle, 9) | Out-Null
  if ($CaptureWidth -gt 0 -and $CaptureHeight -gt 0) {
    [WardianWindowCapture]::SetWindowPos($Handle, $HWND_TOPMOST, 80, 80, $CaptureWidth, $CaptureHeight, 0x0040) | Out-Null
  } else {
    [WardianWindowCapture]::SetWindowPos($Handle, $HWND_TOPMOST, 80, 80, 0, 0, 0x0041) | Out-Null
  }
  [WardianWindowCapture]::SetForegroundWindow($Handle) | Out-Null
  Start-Sleep -Milliseconds 500
  $rect = New-Object WardianWindowCapture+RECT
  [WardianWindowCapture]::GetWindowRect($Handle, [ref]$rect) | Out-Null
  $width = $rect.Right - $rect.Left
  $height = $rect.Bottom - $rect.Top
  if ($width -le 0 -or $height -le 0) {
    throw "Invalid window rectangle ${width}x${height}"
  }

  $bitmap = New-Object System.Drawing.Bitmap $width, $height
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
  } finally {
    if ($CaptureWidth -gt 0 -and $CaptureHeight -gt 0) {
      [WardianWindowCapture]::SetWindowPos($Handle, $HWND_NOTOPMOST, 80, 80, $CaptureWidth, $CaptureHeight, 0x0040) | Out-Null
    } else {
      [WardianWindowCapture]::SetWindowPos($Handle, $HWND_NOTOPMOST, 80, 80, 0, 0, 0x0041) | Out-Null
    }
    $graphics.Dispose()
    $bitmap.Dispose()
  }
}

function Save-WindowTextSnapshot {
  param([IntPtr]$Handle, [string]$Path)
  $previousClipboardText = $null
  $hadClipboardText = $false
  try {
    $previousClipboardText = [System.Windows.Forms.Clipboard]::GetText()
    $hadClipboardText = $true
  } catch {
    $hadClipboardText = $false
  }

  try {
    [WardianWindowCapture]::SetForegroundWindow($Handle) | Out-Null
    Start-Sleep -Milliseconds 250
    [System.Windows.Forms.SendKeys]::SendWait("^+a")
    Start-Sleep -Milliseconds 250
    [System.Windows.Forms.SendKeys]::SendWait("^+c")
    Start-Sleep -Milliseconds 250
    $text = [System.Windows.Forms.Clipboard]::GetText()
    [System.IO.File]::WriteAllText($Path, $text, (New-Object System.Text.UTF8Encoding $false))
  } finally {
    try {
      [System.Windows.Forms.SendKeys]::SendWait("{ESC}")
      Start-Sleep -Milliseconds 150
    } catch {}
    try {
      if ($hadClipboardText) {
        [System.Windows.Forms.Clipboard]::SetText($previousClipboardText)
      } else {
        [System.Windows.Forms.Clipboard]::Clear()
      }
    } catch {}
  }
}

function Send-TerminalText {
  param([IntPtr]$Handle, [string]$Text)
  if ($Text.Trim().Length -le 0) {
    return
  }
  [WardianWindowCapture]::SetForegroundWindow($Handle) | Out-Null
  Start-Sleep -Milliseconds 250
  $rect = New-Object WardianWindowCapture+RECT
  [WardianWindowCapture]::GetWindowRect($Handle, [ref]$rect) | Out-Null
  [WardianWindowCapture]::SetCursorPos($rect.Left + 40, $rect.Top + 300) | Out-Null
  [WardianWindowCapture]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
  [WardianWindowCapture]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 250
  if ($Text -match '^[a-zA-Z0-9 .,_/@:\-]+$') {
    [System.Windows.Forms.SendKeys]::SendWait($Text)
  } else {
    [System.Windows.Forms.Clipboard]::SetText($Text)
    [System.Windows.Forms.SendKeys]::SendWait("^v")
  }
  [System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
  Start-Sleep -Seconds 1
}

function Scroll-TerminalToTop {
  param([IntPtr]$Handle)
  [WardianWindowCapture]::SetForegroundWindow($Handle) | Out-Null
  Start-Sleep -Milliseconds 250
  [System.Windows.Forms.SendKeys]::SendWait("^+{HOME}")
  Start-Sleep -Milliseconds 250
  for ($i = 0; $i -lt 4; $i++) {
    [System.Windows.Forms.SendKeys]::SendWait("^+{PGUP}")
    Start-Sleep -Milliseconds 80
  }
  for ($i = 0; $i -lt 12; $i++) {
    [System.Windows.Forms.SendKeys]::SendWait("^+{UP}")
    Start-Sleep -Milliseconds 30
  }
  $rect = New-Object WardianWindowCapture+RECT
  [WardianWindowCapture]::GetWindowRect($Handle, [ref]$rect) | Out-Null
  $x = [Math]::Max($rect.Left + 40, $rect.Left + [Math]::Floor(($rect.Right - $rect.Left) / 2))
  $y = [Math]::Max($rect.Top + 80, $rect.Top + [Math]::Floor(($rect.Bottom - $rect.Top) / 2))
  [WardianWindowCapture]::SetCursorPos($x, $y) | Out-Null
  for ($i = 0; $i -lt 36; $i++) {
    [WardianWindowCapture]::mouse_event(0x0800, 0, 0, 120, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 20
  }
  Start-Sleep -Milliseconds 500
}

function Send-TerminalFontZoom {
  param([IntPtr]$Handle, [int]$Steps)
  if ($Steps -eq 0) {
    return
  }
  [WardianWindowCapture]::SetForegroundWindow($Handle) | Out-Null
  Start-Sleep -Milliseconds 250
  $key = if ($Steps -lt 0) { "^{SUBTRACT}" } else { "^{ADD}" }
  for ($i = 0; $i -lt [Math]::Abs($Steps); $i++) {
    [System.Windows.Forms.SendKeys]::SendWait($key)
    Start-Sleep -Milliseconds 120
  }
  Start-Sleep -Milliseconds 250
}

$escapedWorkspace = $Workspace.Replace("'", "''")
$providerCwd = $Workspace
$escapedProviderCwd = $escapedWorkspace
$escapedSizeProbePath = $sizeProbePath.Replace("'", "''")
$escapedAnsiQueryPath = $ansiQueryPath.Replace("'", "''")
$escapedStartSignalPath = $startSignalPath.Replace("'", "''")
$escapedProbeScriptPath = (Join-Path $PSScriptRoot "probe-terminal-query.cjs").Replace("'", "''")
$escapedCodexHome = ""
$codexExecutable = "codex"
$claudeExecutable = "claude"
$geminiExecutable = "gemini"
$escapedOpenCodeConfigDir = ""
$escapedOpenCodeConfig = ""
$escapedOpenCodeStateHome = ""
$openCodeProviderSessionId = ""
$claudeSettingsArg = ""
$escapedClaudeSettingsArg = ""
$claudeSettingsFile = ""
$escapedClaudeSettingsFile = ""
$claudePermissionLog = ""
$escapedClaudePermissionLog = ""
$claudeCommonDir = ""
$escapedClaudeCommonDir = ""
$claudeAgentDir = ""
$escapedClaudeAgentDir = ""
$escapedSessionId = $SessionId.Replace("'", "''")
$escapedSessionName = $SessionName.Replace("'", "''")
$escapedOpenCodeTarget = $Workspace.Replace('\', '/').Replace("'", "''")
if ($Provider -eq "codex" -and $WardianHome.Trim().Length -gt 0 -and $SessionId.Trim().Length -gt 0) {
  $escapedCodexHome = (Join-Path $WardianHome (Join-Path "agents" (Join-Path $SessionId "habitat\.codex"))).Replace("'", "''")
}
if ($Provider -eq "codex") {
  $codexCommand = Get-Command "codex.cmd" -ErrorAction SilentlyContinue
  if ($codexCommand -and $codexCommand.Source) {
    $codexExecutable = $codexCommand.Source
  }
}
$escapedCodexExecutable = $codexExecutable.Replace("'", "''")
if ($Provider -eq "claude") {
  $claudeCommand = Get-Command "claude.exe" -ErrorAction SilentlyContinue
  if (-not $claudeCommand) {
    $claudeCommand = Get-Command "claude.cmd" -ErrorAction SilentlyContinue
  }
  if ($claudeCommand -and $claudeCommand.Source) {
    $claudeExecutable = $claudeCommand.Source
  }
}
$escapedClaudeExecutable = $claudeExecutable.Replace("'", "''")
if ($Provider -eq "claude" -and $WardianHome.Trim().Length -gt 0 -and $SessionId.Trim().Length -gt 0) {
  $claudeHookRoot = Join-Path $WardianHome (Join-Path "agents" (Join-Path $SessionId "claude"))
  New-Item -ItemType Directory -Force -Path $claudeHookRoot | Out-Null

  $candidateClaudeCommonDir = Join-Path $WardianHome "common"
  if (Test-Path -LiteralPath $candidateClaudeCommonDir -PathType Container) {
    $claudeCommonDir = $candidateClaudeCommonDir
    $escapedClaudeCommonDir = $claudeCommonDir.Replace("'", "''")
  }
  $candidateClaudeAgentDir = Join-Path $WardianHome (Join-Path "agents" $SessionId)
  if (Test-Path -LiteralPath $candidateClaudeAgentDir -PathType Container) {
    $claudeAgentDir = $candidateClaudeAgentDir
    $escapedClaudeAgentDir = $claudeAgentDir.Replace("'", "''")
  }

  $claudePermissionLog = Join-Path $claudeHookRoot "permission-requests.jsonl"
  [System.IO.File]::WriteAllText($claudePermissionLog, "")

  $claudeHookScript = Join-Path $claudeHookRoot "permission-request-hook.ps1"
  $escapedClaudePermissionLogForScript = $claudePermissionLog.Replace("'", "''")
  $hookScriptContent = @"
`$payload = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace(`$payload)) { exit 0 }
Add-Content -LiteralPath '$escapedClaudePermissionLogForScript' -Value `$payload -Encoding utf8
"@
  [System.IO.File]::WriteAllText(
    $claudeHookScript,
    $hookScriptContent,
    (New-Object System.Text.UTF8Encoding $false)
  )

  $claudeHookCommand = "powershell -WindowStyle Hidden -NoProfile -ExecutionPolicy Bypass -File `"$claudeHookScript`""
  $claudeSettingsArg = ([ordered]@{
    hooks = [ordered]@{
      PermissionRequest = @(
        [ordered]@{
          matcher = "*"
          hooks = @(
            [ordered]@{
              type = "command"
              command = $claudeHookCommand
            }
          )
        }
      )
    }
  } | ConvertTo-Json -Depth 8 -Compress)
  $claudeSettingsFile = Join-Path $claudeHookRoot "claude-settings.json"
  [System.IO.File]::WriteAllText(
    $claudeSettingsFile,
    $claudeSettingsArg,
    (New-Object System.Text.UTF8Encoding $false)
  )
  $escapedClaudeSettingsArg = $claudeSettingsArg.Replace("'", "''")
  $escapedClaudeSettingsFile = $claudeSettingsFile.Replace("'", "''")
  $escapedClaudePermissionLog = $claudePermissionLog.Replace("'", "''")
}
if ($Provider -eq "gemini") {
  $geminiCommand = Get-Command "gemini.cmd" -ErrorAction SilentlyContinue
  if (-not $geminiCommand) {
    $geminiCommand = Get-Command "gemini.exe" -ErrorAction SilentlyContinue
  }
  if ($geminiCommand -and $geminiCommand.Source) {
    $geminiExecutable = $geminiCommand.Source
  }
}
$escapedGeminiExecutable = $geminiExecutable.Replace("'", "''")
if ($Provider -eq "opencode" -and $WardianHome.Trim().Length -gt 0 -and $SessionId.Trim().Length -gt 0) {
  $habitatRoot = Join-Path $WardianHome (Join-Path "agents" (Join-Path $SessionId "habitat"))
  $providerCwd = $habitatRoot
  $escapedProviderCwd = $providerCwd.Replace("'", "''")
  $escapedOpenCodeTarget = (Join-Path $habitatRoot "workspace").Replace('\', '/').Replace("'", "''")
  $opencodeConfigDir = Join-Path $habitatRoot ".opencode"
  $escapedOpenCodeConfigDir = $opencodeConfigDir.Replace("'", "''")
  $escapedOpenCodeConfig = (Join-Path $opencodeConfigDir "opencode.json").Replace("'", "''")
  $opencodeStateHome = Join-Path $WardianHome "xdg-state"
  $opencodeStateDir = Join-Path $opencodeStateHome "opencode"
  New-Item -ItemType Directory -Force -Path $opencodeStateDir | Out-Null
  $opencodeKvPath = Join-Path $opencodeStateDir "kv.json"
  $opencodeKv = [ordered]@{}
  if (Test-Path -LiteralPath $opencodeKvPath -PathType Leaf) {
    try {
      $parsedKv = Get-Content -Raw -LiteralPath $opencodeKvPath | ConvertFrom-Json
      foreach ($property in $parsedKv.PSObject.Properties) {
        $opencodeKv[$property.Name] = $property.Value
      }
    } catch {
      $opencodeKv = [ordered]@{}
    }
  }
  $opencodeKv["tips_hidden"] = $true
  $opencodeKv | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 -LiteralPath $opencodeKvPath
  $escapedOpenCodeStateHome = $opencodeStateHome.Replace("'", "''")

  if ($ProviderSessionId.Trim().StartsWith("ses_")) {
    $openCodeProviderSessionId = $ProviderSessionId.Trim()
  } elseif ($SessionId.Trim().StartsWith("ses_")) {
    $openCodeProviderSessionId = $SessionId.Trim()
  }
  if ($openCodeProviderSessionId.Trim().Length -eq 0) {
    throw "OpenCode outside rendering capture requires ProviderSessionId with the real ses_ provider session."
  }
}
$escapedOpenCodeProviderSessionId = $openCodeProviderSessionId.Replace("'", "''")
$providerInvocation = if ($Provider -eq "codex") {
  "& '$escapedCodexExecutable' -c 'windows.sandbox=""unelevated""' --dangerously-bypass-approvals-and-sandbox --no-alt-screen --cd '$escapedWorkspace' -c tui.show_tooltips=false"
} elseif ($Provider -eq "claude") {
  $claudeSettingsInvocationArg = if ($escapedClaudeSettingsFile.Trim().Length -gt 0) { " --settings '$escapedClaudeSettingsFile'" } else { "" }
  $claudeAddDirInvocationArg = ""
  if ($escapedClaudeCommonDir.Trim().Length -gt 0) {
    $claudeAddDirInvocationArg += " --add-dir '$escapedClaudeCommonDir'"
  }
  if ($escapedClaudeAgentDir.Trim().Length -gt 0) {
    $claudeAddDirInvocationArg += " --add-dir '$escapedClaudeAgentDir'"
  }
  if ($escapedSessionId.Trim().Length -gt 0 -and $escapedSessionName.Trim().Length -gt 0) {
    "& '$escapedClaudeExecutable'$claudeSettingsInvocationArg --verbose --input-format stream-json --output-format stream-json --session-id '$escapedSessionId' --name '$escapedSessionName'$claudeAddDirInvocationArg"
  } elseif ($escapedSessionId.Trim().Length -gt 0) {
    "& '$escapedClaudeExecutable'$claudeSettingsInvocationArg --verbose --input-format stream-json --output-format stream-json --session-id '$escapedSessionId'$claudeAddDirInvocationArg"
  } else {
    "& '$escapedClaudeExecutable'$claudeSettingsInvocationArg --verbose --input-format stream-json --output-format stream-json$claudeAddDirInvocationArg"
  }
} elseif ($Provider -eq "gemini") {
  "& '$escapedGeminiExecutable'"
} elseif ($Provider -eq "opencode" -and $escapedOpenCodeTarget.Trim().Length -gt 0) {
  "& opencode --session '$escapedOpenCodeProviderSessionId' '$escapedOpenCodeTarget'"
} else {
  "& $Provider"
}
$providerCommand = @"
`$Host.UI.RawUI.WindowTitle = '$title'
while (!(Test-Path -LiteralPath '$escapedStartSignalPath')) {
  Start-Sleep -Milliseconds 100
}
`$env:TERM = 'xterm-256color'
`$env:COLORTERM = 'truecolor'
Remove-Item Env:WT_SESSION -ErrorAction SilentlyContinue
Remove-Item Env:WT_PROFILE_ID -ErrorAction SilentlyContinue
if ('$escapedSessionId'.Trim().Length -gt 0) {
  `$env:WARDIAN_SESSION_ID = '$escapedSessionId'
} else {
  Remove-Item Env:WARDIAN_SESSION_ID -ErrorAction SilentlyContinue
}
if ('$Provider' -eq 'claude') {
  `$env:CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD = '1'
} else {
  Remove-Item Env:CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD -ErrorAction SilentlyContinue
}
if ('$escapedCodexHome'.Trim().Length -gt 0) {
  `$env:CODEX_HOME = '$escapedCodexHome'
} else {
  Remove-Item Env:CODEX_HOME -ErrorAction SilentlyContinue
}
if ('$escapedOpenCodeConfigDir'.Trim().Length -gt 0) {
  `$env:OPENCODE_CONFIG_DIR = '$escapedOpenCodeConfigDir'
  `$env:OPENCODE_CONFIG = '$escapedOpenCodeConfig'
} else {
  Remove-Item Env:OPENCODE_CONFIG_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:OPENCODE_CONFIG -ErrorAction SilentlyContinue
}
if ('$escapedOpenCodeStateHome'.Trim().Length -gt 0) {
  `$env:XDG_STATE_HOME = '$escapedOpenCodeStateHome'
} else {
  Remove-Item Env:XDG_STATE_HOME -ErrorAction SilentlyContinue
}
Set-Location -LiteralPath '$escapedProviderCwd'
if ($Columns -gt 0 -and $Rows -gt 0) {
  `$currentBufferSize = `$Host.UI.RawUI.BufferSize
  `$targetBufferWidth = [Math]::Max($Columns, `$currentBufferSize.Width)
  `$targetBufferHeight = [Math]::Max($Rows, 200)
  `$Host.UI.RawUI.BufferSize = New-Object System.Management.Automation.Host.Size(`$targetBufferWidth, `$targetBufferHeight)
  `$Host.UI.RawUI.WindowSize = New-Object System.Management.Automation.Host.Size($Columns, $Rows)
  `$Host.UI.RawUI.BufferSize = New-Object System.Management.Automation.Host.Size($Columns, `$targetBufferHeight)
}
`$wardianWindowSize = `$Host.UI.RawUI.WindowSize
`$wardianBufferSize = `$Host.UI.RawUI.BufferSize
[ordered]@{
  window_width_chars = `$wardianWindowSize.Width
  window_height_chars = `$wardianWindowSize.Height
  buffer_width_chars = `$wardianBufferSize.Width
  buffer_height_chars = `$wardianBufferSize.Height
  term = `$env:TERM
  colorterm = `$env:COLORTERM
  wt_session = `$env:WT_SESSION
  wt_profile_id = `$env:WT_PROFILE_ID
  wardian_session_id = `$env:WARDIAN_SESSION_ID
  claude_additional_directories_claude_md = `$env:CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD
  claude_settings_arg = '$escapedClaudeSettingsArg'
  claude_settings_file = '$escapedClaudeSettingsFile'
  claude_permission_log = '$escapedClaudePermissionLog'
  claude_common_dir = '$escapedClaudeCommonDir'
  claude_agent_dir = '$escapedClaudeAgentDir'
  codex_home = `$env:CODEX_HOME
  codex_executable = '$escapedCodexExecutable'
  claude_executable = '$escapedClaudeExecutable'
  gemini_executable = '$escapedGeminiExecutable'
  font_zoom_steps = $FontZoomSteps
  initial_wait_seconds = $InitialWaitSeconds
  session_name = '$escapedSessionName'
  opencode_config_dir = `$env:OPENCODE_CONFIG_DIR
  opencode_config = `$env:OPENCODE_CONFIG
  opencode_state_home = `$env:XDG_STATE_HOME
  cwd = (Get-Location).Path
} | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -LiteralPath '$escapedSizeProbePath'
if (Get-Command node -ErrorAction SilentlyContinue) {
  & node '$escapedProbeScriptPath' '$escapedAnsiQueryPath' 800
} else {
  [ordered]@{
    error = 'node not found'
  } | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -LiteralPath '$escapedAnsiQueryPath'
}
$providerInvocation
"@

$args = @(
  "-w", $windowName
)
if ($Columns -gt 0 -and $Rows -gt 0) {
  $args += @("--size", "$Columns,$Rows")
}
$args += @(
  "--title", $title,
  "--suppressApplicationTitle",
  "powershell.exe",
  "-NoExit",
  "-NoProfile",
  "-Command", $providerCommand
)

Start-Process -FilePath "wt.exe" -ArgumentList $args | Out-Null
$window = Wait-ProviderWindow -Title $title
$handle = [IntPtr]$window.MainWindowHandle

try {
  if ($captureWindowWidth -gt 0 -and $captureWindowHeight -gt 0) {
    [WardianWindowCapture]::ShowWindow($handle, 9) | Out-Null
    [WardianWindowCapture]::SetWindowPos($handle, [IntPtr]::Zero, 80, 80, $captureWindowWidth, $captureWindowHeight, 0x0040) | Out-Null
    [WardianWindowCapture]::SetForegroundWindow($handle) | Out-Null
    Start-Sleep -Milliseconds 500
  }
  Send-TerminalFontZoom -Handle $handle -Steps $FontZoomSteps
  Set-Content -Encoding UTF8 -LiteralPath $startSignalPath -Value "start"
  Start-Sleep -Seconds $InitialWaitSeconds
    Send-TerminalText -Handle $handle -Text $InputText
    Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "initial.png") -CaptureWidth $captureWindowWidth -CaptureHeight $captureWindowHeight
    Save-WindowTextSnapshot -Handle $handle -Path (Join-Path $outDir "initial.txt")

    if ($resizedCaptureWindowWidth -gt 0 -and $resizedCaptureWindowHeight -gt 0) {
      [WardianWindowCapture]::SetWindowPos($handle, [IntPtr]::Zero, 80, 80, $resizedCaptureWindowWidth, $resizedCaptureWindowHeight, 0x0040) | Out-Null
    }
    Start-Sleep -Seconds 2
    Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "resized.png") -CaptureWidth $resizedCaptureWindowWidth -CaptureHeight $resizedCaptureWindowHeight
    Save-WindowTextSnapshot -Handle $handle -Path (Join-Path $outDir "resized.txt")

    [WardianWindowCapture]::SetForegroundWindow($handle) | Out-Null
    Start-Sleep -Milliseconds 250
    [System.Windows.Forms.SendKeys]::SendWait("^{HOME}")
    Scroll-TerminalToTop -Handle $handle
    Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "scrolled-top.png") -CaptureWidth $resizedCaptureWindowWidth -CaptureHeight $resizedCaptureWindowHeight
    Save-WindowTextSnapshot -Handle $handle -Path (Join-Path $outDir "scrolled-top.txt")

    Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "paused.png") -CaptureWidth $resizedCaptureWindowWidth -CaptureHeight $resizedCaptureWindowHeight
    Save-WindowTextSnapshot -Handle $handle -Path (Join-Path $outDir "paused.txt")

    [WardianWindowCapture]::SetForegroundWindow($handle) | Out-Null
    Start-Sleep -Milliseconds 250
    [System.Windows.Forms.SendKeys]::SendWait("^{c}")
    Start-Sleep -Seconds 1
    Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "interrupted.png") -CaptureWidth $resizedCaptureWindowWidth -CaptureHeight $resizedCaptureWindowHeight
    Save-WindowTextSnapshot -Handle $handle -Path (Join-Path $outDir "interrupted.txt")

  $manifest = [ordered]@{
    provider = $Provider
    run_id = $runId
    workspace = $Workspace
    output_dir = $outDir
    terminal = "Windows Terminal"
    columns = $Columns
    rows = $Rows
    window_width = $captureWindowWidth
    window_height = $captureWindowHeight
    resized_window_width = $resizedCaptureWindowWidth
    resized_window_height = $resizedCaptureWindowHeight
    font_zoom_steps = $FontZoomSteps
    initial_wait_seconds = $InitialWaitSeconds
    terminal_size_probe = $sizeProbePath
    terminal_ansi_query = $ansiQueryPath
    term = "xterm-256color"
    colorterm = "truecolor"
    wardian_home = $WardianHome
    session_id = $SessionId
    provider_session_id = if ($Provider -eq "opencode") { $openCodeProviderSessionId } else { $null }
    used_provider_session_id = if ($Provider -eq "opencode") { $openCodeProviderSessionId } else { $null }
    provider_session_used = if ($Provider -eq "opencode") { $true } else { $false }
    session_name = $SessionName
    codex_home = if ($escapedCodexHome.Trim().Length -gt 0) { $escapedCodexHome } else { $null }
    codex_executable = if ($Provider -eq "codex") { $codexExecutable } else { $null }
    claude_executable = if ($Provider -eq "claude") { $claudeExecutable } else { $null }
    claude_settings_arg = if ($claudeSettingsArg.Trim().Length -gt 0) { $claudeSettingsArg } else { $null }
    claude_settings_file = if ($claudeSettingsFile.Trim().Length -gt 0) { $claudeSettingsFile } else { $null }
    claude_permission_log = if ($claudePermissionLog.Trim().Length -gt 0) { $claudePermissionLog } else { $null }
    claude_common_dir = if ($claudeCommonDir.Trim().Length -gt 0) { $claudeCommonDir } else { $null }
    claude_agent_dir = if ($claudeAgentDir.Trim().Length -gt 0) { $claudeAgentDir } else { $null }
    gemini_executable = if ($Provider -eq "gemini") { $geminiExecutable } else { $null }
    opencode_config_dir = if ($escapedOpenCodeConfigDir.Trim().Length -gt 0) { $escapedOpenCodeConfigDir } else { $null }
    opencode_config = if ($escapedOpenCodeConfig.Trim().Length -gt 0) { $escapedOpenCodeConfig } else { $null }
    opencode_state_home = if ($escapedOpenCodeStateHome.Trim().Length -gt 0) { $escapedOpenCodeStateHome } else { $null }
    provider_cwd = $providerCwd
    provider_invocation = $providerInvocation
    geometry_validation = [ordered]@{
      initial = "probe"
      resized = "evidence_only"
    }
    resized_geometry_note = "The outside resized state is captured as screenshot/text evidence only. This script does not inject ANSI geometry queries into a live provider session because those bytes would be delivered to the provider."
    input_text = $InputText
    input_submitted = ($InputText.Trim().Length -gt 0)
    text_snapshots = @("initial.txt", "resized.txt", "scrolled-top.txt", "paused.txt", "interrupted.txt")
    states = @("initial", "resized", "scrolled-top", "paused", "interrupted")
    note = "External non-Wardian rendering capture. The scrolled-top state uses Ctrl+Home plus repeated mouse-wheel input against Windows Terminal to capture user-visible scrollback. Text snapshots use Windows Terminal select-all/copy after each screenshot; they are machine-readable text evidence but may include scrollback beyond the visible viewport. The paused state captures the unchanged visible buffer before any interrupt input, matching Wardian pause's preserved-terminal-buffer behavior. The interrupted state uses Ctrl+C as a separate provider interruption artifact."
    }
  $manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 (Join-Path $outDir "manifest.json")
  Write-Output $outDir
} finally {
  [WardianWindowCapture]::PostMessage($handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
}
