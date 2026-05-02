<#
.SYNOPSIS
    Installs ScreenRecall Windows build dependencies; optionally Tesseract and Ollama.

.DESCRIPTION
    Uses winget (Windows Package Manager) to install the latest version of:
      - Rust (via rustup)
      - Node.js LTS
      - pnpm
      - Git (if missing)
      - Microsoft Visual Studio 2022 Build Tools (C++ workload) — required by Rust
      - WebView2 Runtime — required by Tauri

    In an interactive console, you are asked whether to install:
      - Tesseract OCR (default offline OCR path)
      - Ollama (skip if using OpenAI-compatible APIs only)

    If Ollama is installed, pulls the default models used out of the box:
      - llama3.2
      - nomic-embed-text

    Safe to re-run; winget no-ops on already-installed items.

.PARAMETER SkipModels
    Skip `ollama pull` calls (only relevant when Ollama is installed).

.PARAMETER SkipBuildTools
    Skip the Visual Studio 2022 Build Tools install (large download).
    Only use this if you already have the C++ desktop workload.

.PARAMETER Tesseract
    Ask | Install | Skip. Default Ask. Skip/Install skips the prompt.

.PARAMETER Ollama
    Ask | Install | Skip. Default Ask. Skip/Install skips the prompt.

.EXAMPLE
    # Interactive: asked about Tesseract and Ollama
    powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1

.EXAMPLE
    # Fully local stack without prompts (CI-friendly)
    powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1 -SkipBuildTools `
        -Tesseract Install -Ollama Install

.EXAMPLE
    # Remote-LLM-only: skip large local backends
    powershell -ExecutionPolicy Bypass -File scripts\setup-windows.ps1 `
        -Tesseract Skip -Ollama Skip
#>

[CmdletBinding()]
param(
    [switch]$SkipModels,
    [switch]$SkipBuildTools,

    [Parameter()]
    [ValidateSet('Ask', 'Install', 'Skip')]
    [string]$Tesseract = 'Ask',

    [Parameter()]
    [ValidateSet('Ask', 'Install', 'Skip')]
    [string]$Ollama = 'Ask'
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) {
    Write-Host ""
    Write-Host "==> $msg" -ForegroundColor Cyan
}

function Write-Ok($msg) {
    Write-Host "    $msg" -ForegroundColor Green
}

function Write-Warn2($msg) {
    Write-Host "    $msg" -ForegroundColor Yellow
}

function Test-PromptHost {
    if ($env:CI -eq 'true' -or $env:GITHUB_ACTIONS -eq 'true') {
        return $false
    }
    try {
        return (-not [Console]::IsInputRedirected) -and [Environment]::UserInteractive
    }
    catch {
        return $false
    }
}

function Read-YesNoPrompt {
    param(
        [Parameter(Mandatory)][string]$Question,
        [bool]$DefaultYes
    )

    $suffix = if ($DefaultYes) { '[Y/n]' } else { '[y/N]' }
    Write-Host ""
    Write-Host "$Question $suffix" -ForegroundColor Cyan
    $resp = Read-Host

    if ([string]::IsNullOrWhiteSpace($resp)) {
        return $DefaultYes
    }
    return (($resp.Trim().Substring(0, 1) -eq 'y') -or ($resp.Trim().Substring(0, 1) -eq 'Y'))
}

function Resolve-IncludeLocal {
    param(
        [Parameter(Mandatory)][ValidateSet('Ask', 'Install', 'Skip')]
        [string]$Mode,

        [Parameter(Mandatory)][string]$ComponentLabel,

        [Parameter(Mandatory)][string]$Explanation,

        # Default Enter = yes vs no
        [Parameter(Mandatory)][bool]$DefaultYes
    )

    if ($Mode -eq 'Install') {
        return $true
    }
    if ($Mode -eq 'Skip') {
        return $false
    }

    if (Test-PromptHost) {
        return (Read-YesNoPrompt -Question "$Explanation" -DefaultYes $DefaultYes)
    }

    Write-Warn2 "Non-interactive session: skipping $ComponentLabel (use -$ComponentLabel Install to force)."
    return $false
}

function Require-Winget {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw @"
winget is not installed. Update 'App Installer' from the Microsoft Store,
or install it from https://aka.ms/getwinget, then re-run this script.
"@
    }
}

function Install-Winget {
    param(
        [Parameter(Mandatory)] [string]$Id,
        [string]$Label = $Id,
        [string[]]$ExtraArgs = @()
    )

    Write-Step "Installing $Label"
    $wingetArgs = @(
        'install',
        '--id', $Id,
        '-e',
        '--accept-package-agreements',
        '--accept-source-agreements',
        '--silent'
    ) + $ExtraArgs

    & winget @wingetArgs
    if ($LASTEXITCODE -eq 0) {
        Write-Ok "$Label installed (or updated)."
    }
    elseif ($LASTEXITCODE -eq -1978335189) {
        # Already installed, nothing to do.
        Write-Ok "$Label already at the latest version."
    }
    else {
        Write-Warn2 "winget returned exit code $LASTEXITCODE for $Label. Continuing."
    }
}

Require-Winget

$wantTesseract = Resolve-IncludeLocal `
    -Mode $Tesseract `
    -ComponentLabel 'Tesseract' `
    -Explanation 'Install Tesseract OCR for the default local OCR pipeline? (Choose No only if you will use another OCR path in Settings.)' `
    -DefaultYes $true

$wantOllama = Resolve-IncludeLocal `
    -Mode $Ollama `
    -ComponentLabel 'Ollama' `
    -Explanation 'Install Ollama for local LLM/embeddings? (Choose No if you will use native or OpenAI-compatible API endpoints instead.)' `
    -DefaultYes $false

# --- Core toolchain -----------------------------------------------------------

Install-Winget -Id 'Git.Git'              -Label 'Git'
Install-Winget -Id 'Rustlang.Rustup'      -Label 'Rust (rustup)'
Install-Winget -Id 'OpenJS.NodeJS.LTS'    -Label 'Node.js LTS'
Install-Winget -Id 'pnpm.pnpm'            -Label 'pnpm'

# --- Native build deps for Tauri ---------------------------------------------

Install-Winget -Id 'Microsoft.EdgeWebView2Runtime' -Label 'WebView2 Runtime'

if (-not $SkipBuildTools) {
    Write-Warn2 "Visual Studio 2022 Build Tools is large (~2-6 GB). Use -SkipBuildTools to skip if you already have the C++ workload."
    # --override passes flags through to the VS installer so it grabs the C++ workload automatically.
    $vsOverride = '--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --includeRecommended'
    Install-Winget -Id 'Microsoft.VisualStudio.2022.BuildTools' `
        -Label 'Visual Studio 2022 Build Tools (C++ workload)' `
        -ExtraArgs @('--override', $vsOverride)
}
else {
    Write-Warn2 "Skipping Visual Studio Build Tools (per -SkipBuildTools)."
}

# --- Optional runtime deps ----------------------------------------------------

if ($wantTesseract) {
    Install-Winget -Id 'UB-Mannheim.TesseractOCR' -Label 'Tesseract OCR'
}
else {
    Write-Warn2 "Skipping Tesseract. Configure OCR under ScreenRecall Settings if needed."
}

if ($wantOllama) {
    Install-Winget -Id 'Ollama.Ollama' -Label 'Ollama'
}
else {
    Write-Warn2 "Skipping Ollama. Point ScreenRecall at an OpenAI-compatible API in Settings if you do not install it locally."
}

# Ensure pnpm is ready even if only Node was pre-existing.
if (Get-Command corepack -ErrorAction SilentlyContinue) {
    Write-Step "Enabling corepack + pnpm@latest"
    corepack enable | Out-Null
    corepack prepare pnpm@latest --activate | Out-Null
    Write-Ok "corepack configured."
}

# --- Model pulls -------------------------------------------------------------

if (-not $SkipModels -and $wantOllama) {
    # Refresh PATH so we can see `ollama` without reopening PowerShell.
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ';' +
                [System.Environment]::GetEnvironmentVariable("Path", "User")

    if (Get-Command ollama -ErrorAction SilentlyContinue) {
        Write-Step "Pulling Ollama models"
        foreach ($model in @('llama3.2', 'nomic-embed-text')) {
            Write-Host "    pulling $model ..." -ForegroundColor DarkGray
            try {
                ollama pull $model
                Write-Ok "$model ready."
            }
            catch {
                Write-Warn2 "Failed to pull ${model}: $_"
            }
        }
    }
    else {
        Write-Warn2 "ollama not on PATH yet - open a new PowerShell window and run:"
        Write-Warn2 '    ollama pull llama3.2; ollama pull nomic-embed-text'
    }
}
elseif ($SkipModels) {
    Write-Warn2 "Skipping ollama pull (per -SkipModels)."
}

# --- Finish ------------------------------------------------------------------

Write-Host ""
Write-Host "All done." -ForegroundColor Green
Write-Host 'Close and reopen PowerShell (so PATH updates) and then run:' -ForegroundColor Green
Write-Host ''
Write-Host ("    cd {0}\.." -f $PSScriptRoot) -ForegroundColor White
Write-Host '    pnpm install' -ForegroundColor White
Write-Host '    pnpm --filter desktop tauri dev' -ForegroundColor White
Write-Host ""
