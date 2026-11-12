param(
    [string]$Version = "",
    [string]$MpvDllPath = "",
    [string]$MpvArchiveUrl = "",
    [string]$MpvArchiveSha256 = "",
    [switch]$SkipInstaller
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

$RootDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$CargoToml = Join-Path $RootDir "crates\app\Cargo.toml"

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Match = Select-String -Path $CargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $Match) {
        throw "Unable to read version from crates\app\Cargo.toml"
    }
    $Version = $Match.Matches[0].Groups[1].Value
}
$Version = $Version.TrimStart("v", "V")

$DistDir = Join-Path $RootDir "dist"
$StageDir = Join-Path $DistDir "win-stage"
if (Test-Path $StageDir) {
    Remove-Item -Recurse -Force $StageDir
}
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

Write-Host "Building Ferrite v$Version (x86_64-pc-windows-msvc)..."
Push-Location $RootDir
try {
    cargo build --release -p ferrite
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release -p ferrite failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$ExePath = Join-Path $RootDir "target\release\ferrite.exe"
if (-not (Test-Path $ExePath)) {
    throw "Expected binary not found at $ExePath"
}
Copy-Item -Path $ExePath -Destination (Join-Path $StageDir "ferrite.exe") -Force

$ResolvedDll = ""
if (-not [string]::IsNullOrWhiteSpace($MpvDllPath)) {
    if (-not (Test-Path $MpvDllPath)) {
        throw "Provided -MpvDllPath does not exist: $MpvDllPath"
    }
    $ResolvedDll = (Resolve-Path $MpvDllPath).Path
} elseif (-not [string]::IsNullOrWhiteSpace($MpvArchiveUrl)) {
    $ParsedUri = [System.Uri]$MpvArchiveUrl
    if ($ParsedUri.Scheme -ne "https") {
        throw "Refusing non-HTTPS libmpv archive URL: $MpvArchiveUrl"
    }
    if ([string]::IsNullOrWhiteSpace($MpvArchiveSha256)) {
        throw "-MpvArchiveSha256 is required when downloading -MpvArchiveUrl"
    }

    $TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("ferrite-mpv-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
    $ArchiveFile = Join-Path $TempDir (Split-Path $ParsedUri.AbsolutePath -Leaf)

    Invoke-WebRequest -Uri $MpvArchiveUrl -OutFile $ArchiveFile -UseBasicParsing
    $ActualHash = (Get-FileHash -Path $ArchiveFile -Algorithm SHA256).Hash.ToLowerInvariant()
    $ExpectedHash = $MpvArchiveSha256.Trim().ToLowerInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        Remove-Item -Recurse -Force $TempDir
        throw "SHA-256 mismatch for downloaded libmpv archive (expected $ExpectedHash, got $ActualHash)"
    }

    $ExtractDir = Join-Path $TempDir "extracted"
    New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null
    if ($ArchiveFile.EndsWith(".zip")) {
        Expand-Archive -Path $ArchiveFile -DestinationPath $ExtractDir -Force
    } else {
        & 7z x $ArchiveFile "-o$ExtractDir" -y | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "7z extraction failed for $ArchiveFile"
        }
    }

    $FoundDll = Get-ChildItem -Path $ExtractDir -Filter "libmpv-2.dll" -Recurse | Select-Object -First 1
    if (-not $FoundDll) {
        throw "libmpv-2.dll not found inside extracted archive"
    }
    $ResolvedDll = $FoundDll.FullName
} else {
    throw "Pass -MpvDllPath <path\to\libmpv-2.dll> or both -MpvArchiveUrl and -MpvArchiveSha256"
}

Copy-Item -Path $ResolvedDll -Destination (Join-Path $StageDir "libmpv-2.dll") -Force
Copy-Item -Path (Join-Path $RootDir "LICENSE") -Destination (Join-Path $StageDir "LICENSE") -Force
Copy-Item -Path (Join-Path $RootDir "THIRD_PARTY_NOTICES.md") -Destination (Join-Path $StageDir "THIRD_PARTY_NOTICES.md") -Force

$PortableZip = Join-Path $DistDir "ferrite-windows-x86_64.zip"
if (Test-Path $PortableZip) {
    Remove-Item -Force $PortableZip
}
Compress-Archive -Path (Join-Path $StageDir "*") -DestinationPath $PortableZip
Write-Host "Built portable archive: $PortableZip"

if (-not $SkipInstaller) {
    $IsccCandidates = @(
        "ISCC.exe",
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe")
    )
    $IsccExe = $null
    foreach ($Candidate in $IsccCandidates) {
        if (-not [string]::IsNullOrWhiteSpace($Candidate)) {
            $Cmd = Get-Command $Candidate -ErrorAction SilentlyContinue
            if ($Cmd) {
                $IsccExe = $Cmd.Source
                break
            }
            if (Test-Path $Candidate) {
                $IsccExe = $Candidate
                break
            }
        }
    }
    if (-not $IsccExe) {
        throw "Inno Setup 6 compiler (ISCC.exe) not found. Install Inno Setup 6 or pass -SkipInstaller."
    }

    $IssPath = Join-Path $RootDir "packaging\windows\ferrite.iss"
    & $IsccExe "/DMyAppVersion=$Version" "/DStageDir=$StageDir" $IssPath
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC.exe failed with exit code $LASTEXITCODE"
    }
    Write-Host "Built Windows installer: $(Join-Path $DistDir "Ferrite-Setup-$Version-x64.exe")"
}
