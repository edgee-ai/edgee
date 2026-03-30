#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$InstallDir = ""
)

$ErrorActionPreference = 'Stop'

$GithubOwner = 'edgee-ai'
$GithubRepo  = 'edgee'
$Target      = 'x86_64-pc-windows-msvc'
$BinaryName  = 'edgee.exe'

# ── Formatting helpers ──────────────────────────────────────────────────────

function Write-Header {
    Write-Host ""
    Write-Host "       " -NoNewline; Write-Host "◢████◤" -ForegroundColor White
    Write-Host "   " -NoNewline; Write-Host "" -ForegroundColor White
    Write-Host "  " -NoNewline; Write-Host "◢██████◤" -ForegroundColor White
    Write-Host "  " -NoNewline; Write-Host "" -ForegroundColor White
    Write-Host "  " -NoNewline; Write-Host "◢████████◤" -ForegroundColor White
    Write-Host ""
    Write-Host "  Token compression gateway for Claude Code, Codex & Opencode" -ForegroundColor DarkGray
    Write-Host "  https://www.edgee.ai" -ForegroundColor DarkGray
    Write-Host ""
}

function Write-Step([string]$msg) {
    Write-Host "  " -NoNewline
    Write-Host "→" -ForegroundColor Cyan -NoNewline
    Write-Host " $msg"
}

function Write-Ok([string]$msg) {
    Write-Host "  " -NoNewline
    Write-Host "✓" -ForegroundColor Green -NoNewline
    Write-Host " $msg"
}

function Write-Err([string]$msg) {
    Write-Host ""
    Write-Host "  " -NoNewline
    Write-Host "✗ Error:" -ForegroundColor Red -NoNewline
    Write-Host " $msg"
    Write-Host ""
    exit 1
}

# ── Installation logic ──────────────────────────────────────────────────────

function Get-InstallDir {
    if ($InstallDir -ne "") { return $InstallDir }
    if ($env:INSTALL_DIR -ne $null -and $env:INSTALL_DIR -ne "") { return $env:INSTALL_DIR }
    return Join-Path $env:LOCALAPPDATA "Programs\edgee"
}

function Get-LatestVersion {
    $apiUrl = "https://api.github.com/repos/$GithubOwner/$GithubRepo/releases/latest"
    try {
        $response = Invoke-RestMethod -Uri $apiUrl -Headers @{ 'User-Agent' = 'edgee-installer' }
        return $response.tag_name
    } catch {
        Write-Err "Failed to fetch latest release info: $_"
    }
}

function Get-Sha256([string]$FilePath) {
    $hash = Get-FileHash -Path $FilePath -Algorithm SHA256
    return $hash.Hash.ToLower()
}

function Install-Edgee {
    Write-Header

    $targetInstallDir = Get-InstallDir
    $version = Get-LatestVersion
    $baseUrl = "https://github.com/$GithubOwner/$GithubRepo/releases/download/$version"
    $remoteFile = "edgee.$Target.exe"
    $checksumFile = "edgee.$Target.exe.sha256"

    Write-Host ("  {0,-9} Windows (x86_64)" -f "Platform") -ForegroundColor White
    Write-Host ("  {0,-9} $targetInstallDir" -f "Directory") -ForegroundColor White
    Write-Host ""

    $tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $tmpDir | Out-Null

    try {
        # Download binary
        Write-Step "Downloading binary..."
        $binaryTmp = Join-Path $tmpDir $BinaryName
        try {
            Invoke-WebRequest -Uri "$baseUrl/$remoteFile" -OutFile $binaryTmp -UseBasicParsing
        } catch {
            Write-Err "Failed to download binary: $_"
        }
        Write-Ok "Binary downloaded"

        # Download checksum
        Write-Step "Downloading checksum..."
        $checksumTmp = Join-Path $tmpDir "edgee.sha256"
        try {
            Invoke-WebRequest -Uri "$baseUrl/$checksumFile" -OutFile $checksumTmp -UseBasicParsing
        } catch {
            Write-Err "Failed to download checksum: $_"
        }
        Write-Ok "Checksum downloaded"

        # Verify checksum
        Write-Step "Verifying integrity..."
        $expected = (Get-Content $checksumTmp).Trim().ToLower()
        $actual = Get-Sha256 $binaryTmp
        if ($expected -ne $actual) {
            Write-Err "Checksum mismatch!`n  Expected: $expected`n  Got:      $actual"
        }
        Write-Ok "Checksum verified"

        # Install binary
        Write-Step "Installing to $targetInstallDir..."
        if (-not (Test-Path $targetInstallDir)) {
            New-Item -ItemType Directory -Path $targetInstallDir -Force | Out-Null
        }
        $dest = Join-Path $targetInstallDir $BinaryName
        Copy-Item -Path $binaryTmp -Destination $dest -Force

        $installedVersion = & $dest --version 2>&1
        $versionNum = ($installedVersion -split ' ')[1]
        Write-Ok "Installed edgee v$versionNum"

    } finally {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "  ╔═══════════════════════════════════════════════╗"
    Write-Host "  ║  " -NoNewline
    Write-Host ("Edgee v$versionNum installed successfully!") -ForegroundColor Green -NoNewline
    Write-Host "$((' ' * [Math]::Max(0, 43 - "Edgee v$versionNum installed successfully!".Length)))║"
    Write-Host "  ╚═══════════════════════════════════════════════╝"

    Write-Host ""
    Write-Host "  Get started:" -ForegroundColor White
    Write-Host ""
    Write-Host "    " -NoNewline; Write-Host "edgee auth login" -ForegroundColor Cyan -NoNewline
    Write-Host "    # authenticate with your Edgee account" -ForegroundColor DarkGray
    Write-Host "    " -NoNewline; Write-Host "edgee launch claude" -ForegroundColor Cyan -NoNewline
    Write-Host "  # launch Claude Code with token compression" -ForegroundColor DarkGray
    Write-Host "    " -NoNewline; Write-Host "edgee --help" -ForegroundColor Cyan -NoNewline
    Write-Host "         # show all available commands" -ForegroundColor DarkGray
    Write-Host ""

    # Add install dir to PATH permanently (User scope, no admin required)
    $registryPath = [System.Environment]::GetEnvironmentVariable('PATH', 'User')
    $registryDirs = if ($registryPath) { $registryPath -split ';' } else { @() }
    if ($registryDirs -notcontains $targetInstallDir) {
        $newPath = ($registryDirs + $targetInstallDir | Where-Object { $_ -ne '' }) -join ';'
        [System.Environment]::SetEnvironmentVariable('PATH', $newPath, 'User')
        # Also update current session so edgee is usable immediately
        $env:PATH = "$env:PATH;$targetInstallDir"
        Write-Ok "Added to PATH"
        Write-Host ""
        Write-Host "  " -NoNewline
        Write-Host "Open a new terminal" -ForegroundColor Yellow -NoNewline
        Write-Host " to use " -NoNewline
        Write-Host "edgee" -ForegroundColor Cyan -NoNewline
        Write-Host " globally."
        Write-Host ""
    } else {
        Write-Ok "Already in PATH"
        Write-Host ""
    }
}

Install-Edgee
