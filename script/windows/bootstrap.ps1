#!/usr/bin/env powershell
param(
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Show-Usage {
    Write-Output 'Usage: .\script\windows\bootstrap.ps1 [-Help]'
    Write-Output ''
    Write-Output 'Prepare this checkout for Smash development on Windows.'
    Write-Output ''
    Write-Output 'Options:'
    Write-Output '  -Help                 Show this help message.'
}


function Show-BootstrapPreview {
    Write-Output 'Smash bootstrap is starting for Windows.'
    Write-Output 'It will:'
    Write-Output '  - Check for Git for Windows.'
    Write-Output '  - Install Rust if cargo is unavailable.'
    Write-Output '  - Install Visual Studio Build Tools, jq, CMake, Protobuf, LLVM, InnoSetup, and gcloud as needed.'
    Write-Output '  - Install Cargo test dependencies.'

    Write-Output 'Run .\script\windows\bootstrap.ps1 -Help to see options and environment overrides.'
    Write-Output ''
}

function Add-DirectoryToPathIfPresent {
    param([string]$Path)

    if (-not $Path -or -not (Test-Path -Path $Path -PathType Container)) {
        return
    }

    $pathEntries = $env:PATH -split ';'
    if ($pathEntries -notcontains $Path) {
        $env:PATH = "$Path;$env:PATH"
    }
}

function Add-WinGetPackageCommandToPath {
    param(
        [string]$CommandName,
        [string]$PackageId
    )

    if (Get-Command -Name $CommandName -Type Application -ErrorAction SilentlyContinue) {
        return
    }

    $winGetPackagesDir = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    if (-not (Test-Path -Path $winGetPackagesDir -PathType Container)) {
        return
    }

    $escapedPackageId = [WildcardPattern]::Escape($PackageId)
    $packageDirs = Get-ChildItem -Path $winGetPackagesDir -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "$escapedPackageId*" }

    foreach ($packageDir in $packageDirs) {
        $command = Get-ChildItem -Path $packageDir.FullName -Filter "$CommandName.exe" -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($command) {
            Add-DirectoryToPathIfPresent $command.DirectoryName
            return
        }
    }
}

function Use-LibclangIfInstalled {
    $candidateDirs = @(
        "$env:ProgramFiles\LLVM\bin",
        "${env:ProgramFiles(x86)}\LLVM\bin"
    )

    foreach ($dir in $candidateDirs) {
        if (Test-Path -Path (Join-Path $dir 'libclang.dll') -PathType Leaf) {
            Add-DirectoryToPathIfPresent $dir
            $env:LIBCLANG_PATH = $dir
            return
        }
    }

    $winGetPackagesDir = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    if (-not (Test-Path -Path $winGetPackagesDir -PathType Container)) {
        return
    }

    $packageDirs = Get-ChildItem -Path $winGetPackagesDir -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like 'LLVM.LLVM*' }

    foreach ($packageDir in $packageDirs) {
        $libclang = Get-ChildItem -Path $packageDir.FullName -Filter 'libclang.dll' -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($libclang) {
            Add-DirectoryToPathIfPresent $libclang.DirectoryName
            $env:LIBCLANG_PATH = $libclang.DirectoryName
            return
        }
    }
}

if ($Help) {
    Show-Usage
    exit 0
}
Show-BootstrapPreview

# Git for Windows can be installed system-wide (Program Files) or per-user (LOCALAPPDATA\Programs\Git).
$gitBinCandidates = @(
    "$env:PROGRAMFILES\Git\bin",
    "$env:LOCALAPPDATA\Programs\Git\bin"
)
$gitBinDir = $gitBinCandidates | Where-Object { Test-Path -PathType Container $_ } | Select-Object -First 1
if (-not $gitBinDir) {
    Write-Error 'Git for Windows is required. Please install it at:'
    Write-Error 'https://gitforwindows.org/'
    exit 1
}
Add-DirectoryToPathIfPresent $gitBinDir

# Some Rust build scripts depend on Unix patch.exe, which ships with Git for Windows.
$gitUsrBinDir = Join-Path (Split-Path -Parent $gitBinDir) 'usr\bin'
Add-DirectoryToPathIfPresent $gitUsrBinDir

if (-not (Get-Command -Name cargo -Type Application -ErrorAction SilentlyContinue)) {
    Write-Output 'Installing rust...'
    Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile "$env:Temp\rustup-init.exe"
    & "$env:Temp\rustup-init.exe"
    Write-Output 'Please start a new terminal session so that cargo is in your PATH'
    exit 1
}

# Visual Studio Build Tools (MSVC compiler + linker + Windows SDK) are required to link Rust crates
# targeting x86_64-pc-windows-msvc.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$haveMsvcBuildTools = $false
if (Test-Path $vswhere) {
    $vsInstall = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 Microsoft.VisualStudio.Component.Windows11SDK.22621 `
        -property installationPath
    if ($vsInstall) { $haveMsvcBuildTools = $true }
}
if (-not $haveMsvcBuildTools) {
    Write-Output 'Installing Visual Studio Build Tools (MSVC + Windows SDK)...'
    winget install -e --id Microsoft.VisualStudio.2022.BuildTools `
        --accept-package-agreements --accept-source-agreements `
        --override '--passive --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --includeRecommended'
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# A bash executable should come with Git for Windows
& "$gitBinDir\bash.exe" "$PWD\script\install_cargo_test_deps"

# Needed in wasm compilation for parsing the version of wasm-bindgen
winget install jqlang.jq

# CMake is needed to build some dependencies, e.g.: sentry-contrib-native.
winget install -e --id Kitware.CMake

# Protoc is required by prost-build for warp-proto-apis generated crates.
winget install -e --id Google.Protobuf
Add-WinGetPackageCommandToPath -CommandName 'protoc' -PackageId 'Google.Protobuf'

# LLVM provides libclang.dll, which is required by bindgen-based build scripts.
winget install -e --id LLVM.LLVM
Use-LibclangIfInstalled

# We use InnoSetup to build our release bundle installer.
winget install -e --id JRSoftware.InnoSetup

# If we don't see gcloud command, try adding the install location to the PATH.
if (-not (Get-Command -Name gcloud -Type Application -ErrorAction SilentlyContinue)) {
    $env:PATH += ";$env:LOCALAPPDATA\Google\Cloud SDK\google-cloud-sdk\bin"
}

# If we still don't see it, install it.
if (-not (Get-Command -Name gcloud -Type Application -ErrorAction SilentlyContinue)) {
    (New-Object Net.WebClient).DownloadFile('https://dl.google.com/dl/cloudsdk/channels/rapid/GoogleCloudSDKInstaller.exe', "$env:Temp\GoogleCloudSDKInstaller.exe")
    Start-Process "$env:Temp\GoogleCloudSDKInstaller.exe" -Wait
}

if ($env:SMASH_SKIP_GCLOUD_AUTH -ne '1') {
    [string]$identityToken = gcloud auth print-identity-token
    if ($identityToken.Trim().Length -eq 0) {
        Write-Output 'gcloud CLI authentication missing.  Press enter to continue...'
        Read-Host
        gcloud auth login
    }
}
