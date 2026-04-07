Param(
  [switch]$SkipTauriDriver,
  [switch]$SkipEdgeDriver
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$NativeToolsDir = Join-Path $RepoRoot "tools\e2e-native"

function Ensure-NativeToolsDir {
  if (-not (Test-Path $NativeToolsDir)) {
    New-Item -ItemType Directory -Path $NativeToolsDir | Out-Null
  }
}

function Require-Command {
  param([string]$Name)
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "Required command '$Name' was not found on PATH."
  }
}

Require-Command cargo

if (-not $SkipTauriDriver) {
  $tauriDriver = Get-Command tauri-driver -ErrorAction SilentlyContinue
  if (-not $tauriDriver) {
    Write-Host "Installing tauri-driver..."
    cargo install tauri-driver --locked
  } else {
    Write-Host "tauri-driver already installed at $($tauriDriver.Source)"
  }
}

if (-not $SkipEdgeDriver) {
  Ensure-NativeToolsDir
  $edgeDriver = Get-Command msedgedriver.exe -ErrorAction SilentlyContinue
  if (-not $edgeDriver) {
    $tool = Get-Command msedgedriver-tool.exe -ErrorAction SilentlyContinue
    if (-not $tool) {
      Write-Host "Installing msedgedriver-tool..."
      cargo install --git https://github.com/chippers/msedgedriver-tool
      $tool = Get-Command msedgedriver-tool.exe -ErrorAction SilentlyContinue
    }

    if (-not $tool) {
      throw "msedgedriver-tool.exe was not found after installation."
    }

    Write-Host "Downloading matching msedgedriver.exe..."
    Push-Location $NativeToolsDir
    try {
      & $tool.Source
    } finally {
      Pop-Location
    }
  } else {
    Write-Host "msedgedriver already installed at $($edgeDriver.Source)"
  }
}

Write-Host ""
Write-Host "Native E2E prerequisites are prepared."
Write-Host "Recommended next steps:"
Write-Host "  npm run test:e2e:native"
Write-Host "  `$env:WARDIAN_E2E_REAL_OPENCODE='1'; npm run test:e2e:native"
