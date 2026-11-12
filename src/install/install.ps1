# Installs Hufu on Windows.
#
#   irm https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.ps1 | iex
#
# To pass options, download the script first or set the environment variables:
#   -Up                run `hufu up` afterwards                             ($env:HUFU_UP = "1")
#   -Version <tag>     install this release instead of the latest one       ($env:HUFU_VERSION)
#   -Dir <path>        install here, default %LOCALAPPDATA%\hufu\bin        ($env:HUFU_INSTALL_DIR)
#   -FromSource        build with cargo instead of downloading a binary     ($env:HUFU_FROM_SOURCE = "1")
#
# $env:HUFU_DOWNLOAD_BASE = "<url>" downloads the archive and its checksum from a mirror instead of GitHub.
#
# The script downloads one archive from the project's GitHub releases, checks its SHA-256 sum,
# copies one file, hufu.exe, into the install directory and adds that directory to your user PATH.

param(
    [switch]$Up,
    [string]$Version = $(if ($env:HUFU_VERSION) { $env:HUFU_VERSION } else { "latest" }),
    [string]$Dir = $(if ($env:HUFU_INSTALL_DIR) { $env:HUFU_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "hufu\bin" }),
    [switch]$FromSource
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$Repo = "kverona-ai/hufu"
if ($env:HUFU_UP -eq "1") { $Up = $true }
if ($env:HUFU_FROM_SOURCE -eq "1") { $FromSource = $true }

function Get-Target {
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "ARM64" { return "aarch64-pc-windows-msvc" }
        default { return $null }
    }
}

function Install-FromSource {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "building from source needs Rust: install it from https://rustup.rs and run this again"
    }
    Write-Host "Building Hufu from source with cargo (this takes a minute)..."
    if ((Split-Path $Dir -Leaf) -eq "bin") {
        & cargo install --git "https://github.com/$Repo" hufu-cli --locked --force --root (Split-Path $Dir -Parent)
    } else {
        & cargo install --git "https://github.com/$Repo" hufu-cli --locked --force
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
        $script:Dir = Join-Path $cargoHome "bin"
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo install failed" }
}

function Install-FromRelease {
    $target = Get-Target
    if (-not $target) { return $false }
    if ($env:HUFU_DOWNLOAD_BASE) {
        $base = $env:HUFU_DOWNLOAD_BASE.TrimEnd("/")
    } elseif ($Version -eq "latest") {
        $base = "https://github.com/$Repo/releases/latest/download"
    } else {
        $base = "https://github.com/$Repo/releases/download/$Version"
    }
    $asset = "hufu-$target.zip"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hufu-install-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "Downloading $asset ($Version)..."
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset)
        } catch {
            return $false
        }
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset.sha256" -OutFile (Join-Path $tmp "$asset.sha256")
        } catch {
            throw "the release has no checksum for $asset; not installing an unverified binary"
        }
        $want = ((Get-Content (Join-Path $tmp "$asset.sha256") -Raw).Trim() -split "\s+")[0].ToLowerInvariant()
        $got = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLowerInvariant()
        if ($want -ne $got) { throw "checksum mismatch for ${asset}: expected $want, got $got" }
        Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
        New-Item -ItemType Directory -Force -Path $Dir | Out-Null
        $dest = Join-Path $Dir "hufu.exe"
        if (Test-Path $dest) {
            # A running hufu.exe cannot be overwritten on Windows, but it can be renamed out of the way.
            $old = Join-Path $Dir ("hufu.old-" + [System.Guid]::NewGuid().ToString("N") + ".exe")
            Move-Item -Force $dest $old
            Remove-Item -Force $old -ErrorAction SilentlyContinue
        }
        Get-ChildItem $Dir -Filter "hufu.old-*.exe" -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        Copy-Item (Join-Path $tmp "hufu-$target\hufu.exe") $dest
        return $true
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

if ($FromSource) {
    Install-FromSource
} elseif (-not (Install-FromRelease)) {
    Write-Host "No prebuilt binary could be downloaded for this machine ($env:PROCESSOR_ARCHITECTURE, $Version)."
    Install-FromSource
}

$exe = Join-Path $Dir "hufu.exe"
if (-not (Test-Path $exe)) { throw "the install did not produce $exe" }
Write-Host "Installed $(& $exe --version) to $exe"

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$onPath = @($userPath -split ";" | Where-Object { $_ }) -contains $Dir
if (-not $onPath) {
    $newPath = if ($userPath) { "$userPath;$Dir" } else { $Dir }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = "$env:Path;$Dir"
    Write-Host "Added $Dir to your user PATH. Open a new terminal for other programs to see it."
}

Write-Host ""
if ($Up) {
    & $exe up
} else {
    Write-Host "Next: run ``hufu up`` to configure Hufu, register it with your agents and start the daemon."
}
