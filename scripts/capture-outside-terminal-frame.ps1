param(
  [string]$OutputRoot = (Join-Path (Get-Location).Path "e2e\screenshots\outside-terminal-frame"),
  [int]$InitialWaitSeconds = 2,
  [int]$Columns = 50,
  [int]$Rows = 19,
  [int]$WindowWidth = 0,
  [int]$WindowHeight = 0,
  [int]$FontZoomSteps = 0
)

$ErrorActionPreference = "Stop"

$runId = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH-mm-ssZ")
$title = "WardianOutsideFrame-$runId"
$windowName = "wardian-outside-frame-$runId"
$outDir = Join-Path $OutputRoot $runId
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$frameScript = Join-Path $outDir "frame.ps1"
$sizeProbePath = Join-Path $outDir "terminal-size.json"
$ansiQueryPath = Join-Path $outDir "terminal-ansi-query.json"
$startSignalPath = Join-Path $outDir "start.signal"

$frameScriptContent = @'
$block = [char]0x2590
$check = [char]0x2713
$omega = [char]0x03A9
$lines = 1..42 | ForEach-Object {
  "render-{0:D2} | $block glyph | $check check | cafe | omega $omega" -f $_
}
$esc = [char]27
$frame = "$esc[2J$esc[H$(($lines -join "`r`n"))`r`n"
[Console]::Write($frame)
while ($true) {
  Start-Sleep -Seconds 1
}
'@
Set-Content -Encoding UTF8 -LiteralPath $frameScript -Value $frameScriptContent

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WardianFrameWindowCapture {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@

function Wait-FrameWindow {
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
  param([IntPtr]$Handle, [string]$Path)
  $HWND_TOPMOST = [IntPtr](-1)
  $HWND_NOTOPMOST = [IntPtr](-2)
  [WardianFrameWindowCapture]::ShowWindow($Handle, 9) | Out-Null
  if ($WindowWidth -gt 0 -and $WindowHeight -gt 0) {
    [WardianFrameWindowCapture]::SetWindowPos($Handle, $HWND_TOPMOST, 80, 80, $WindowWidth, $WindowHeight, 0x0040) | Out-Null
  } else {
    [WardianFrameWindowCapture]::SetWindowPos($Handle, $HWND_TOPMOST, 80, 80, 0, 0, 0x0041) | Out-Null
  }
  [WardianFrameWindowCapture]::SetForegroundWindow($Handle) | Out-Null
  Start-Sleep -Milliseconds 500

  $rect = New-Object WardianFrameWindowCapture+RECT
  [WardianFrameWindowCapture]::GetWindowRect($Handle, [ref]$rect) | Out-Null
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
    if ($WindowWidth -gt 0 -and $WindowHeight -gt 0) {
      [WardianFrameWindowCapture]::SetWindowPos($Handle, $HWND_NOTOPMOST, 80, 80, $WindowWidth, $WindowHeight, 0x0040) | Out-Null
    } else {
      [WardianFrameWindowCapture]::SetWindowPos($Handle, $HWND_NOTOPMOST, 80, 80, 0, 0, 0x0041) | Out-Null
    }
    $graphics.Dispose()
    $bitmap.Dispose()
  }
}

function Send-TerminalFontZoom {
  param([IntPtr]$Handle, [int]$Steps)
  if ($Steps -eq 0) {
    return
  }
  [WardianFrameWindowCapture]::SetForegroundWindow($Handle) | Out-Null
  Start-Sleep -Milliseconds 250
  $key = if ($Steps -lt 0) { "^{SUBTRACT}" } else { "^{ADD}" }
  for ($i = 0; $i -lt [Math]::Abs($Steps); $i++) {
    [System.Windows.Forms.SendKeys]::SendWait($key)
    Start-Sleep -Milliseconds 120
  }
  Start-Sleep -Milliseconds 250
}

$escapedFrameScript = $frameScript.Replace("'", "''")
$escapedSizeProbePath = $sizeProbePath.Replace("'", "''")
$escapedAnsiQueryPath = $ansiQueryPath.Replace("'", "''")
$escapedStartSignalPath = $startSignalPath.Replace("'", "''")
$escapedProbeScriptPath = (Join-Path $PSScriptRoot "probe-terminal-query.cjs").Replace("'", "''")
$providerCommand = @"
`$Host.UI.RawUI.WindowTitle = '$title'
while (!(Test-Path -LiteralPath '$escapedStartSignalPath')) {
  Start-Sleep -Milliseconds 100
}
`$OutputEncoding = [System.Text.UTF8Encoding]::new()
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
`$env:TERM = 'xterm-256color'
`$env:COLORTERM = 'truecolor'
chcp 65001 > `$null
if ($Columns -gt 0 -and $Rows -gt 0) {
  `$Host.UI.RawUI.WindowSize = New-Object System.Management.Automation.Host.Size($Columns, $Rows)
  `$Host.UI.RawUI.BufferSize = New-Object System.Management.Automation.Host.Size($Columns, 200)
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
  font_zoom_steps = $FontZoomSteps
} | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -LiteralPath '$escapedSizeProbePath'
if (Get-Command node -ErrorAction SilentlyContinue) {
  & node '$escapedProbeScriptPath' '$escapedAnsiQueryPath' 800
} else {
  [ordered]@{
    error = 'node not found'
  } | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -LiteralPath '$escapedAnsiQueryPath'
}
. '$escapedFrameScript'
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
$window = Wait-FrameWindow -Title $title
$handle = [IntPtr]$window.MainWindowHandle

try {
  Send-TerminalFontZoom -Handle $handle -Steps $FontZoomSteps
  Set-Content -Encoding UTF8 -LiteralPath $startSignalPath -Value "start"
  Start-Sleep -Seconds $InitialWaitSeconds
  Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "initial.png")

  if ($WindowWidth -gt 0 -and $WindowHeight -gt 0) {
    [WardianFrameWindowCapture]::SetWindowPos($handle, [IntPtr]::Zero, 80, 80, $WindowWidth, $WindowHeight, 0x0040) | Out-Null
  }
  Start-Sleep -Seconds 2
  Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "resized.png")

  [WardianFrameWindowCapture]::SetForegroundWindow($handle) | Out-Null
  Start-Sleep -Milliseconds 250
  [System.Windows.Forms.SendKeys]::SendWait("^{HOME}")
  Start-Sleep -Seconds 1
  Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "scrolled-top.png")

  [WardianFrameWindowCapture]::SetForegroundWindow($handle) | Out-Null
  Start-Sleep -Milliseconds 250
  [System.Windows.Forms.SendKeys]::SendWait("^{c}")
  Start-Sleep -Seconds 1
  Save-WindowScreenshot -Handle $handle -Path (Join-Path $outDir "interrupted.png")

  $manifest = [ordered]@{
    run_id = $runId
    output_dir = $outDir
    terminal = "Windows Terminal"
    columns = $Columns
    rows = $Rows
    window_width = $WindowWidth
    window_height = $WindowHeight
    font_zoom_steps = $FontZoomSteps
    term = "xterm-256color"
    colorterm = "truecolor"
    terminal_size_probe = $sizeProbePath
    terminal_ansi_query = $ansiQueryPath
    frame_script = $frameScript
    states = @("initial", "resized", "scrolled-top", "interrupted")
    note = "External Windows Terminal capture of the deterministic Wardian terminal rendering frame."
  }
  $manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 (Join-Path $outDir "manifest.json")
  Write-Output $outDir
} finally {
  [WardianFrameWindowCapture]::PostMessage($handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
}
