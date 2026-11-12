# Developer helper: builds Reinstall Kit into dist\ReinstallKit (and the installer with -Installer).
# Needs Rust (https://rustup.rs). With the GNU toolchain a full MinGW-w64 (for example WinLibs) must be on PATH.
#   powershell -ExecutionPolicy Bypass -File build.ps1
#   powershell -ExecutionPolicy Bypass -File build.ps1 -Installer     (needs Inno Setup 6)
param([switch]$Installer)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo -and $env:CARGO_HOME) { $cargo = Join-Path $env:CARGO_HOME 'bin\cargo.exe' }
if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path -LiteralPath $cargo)) { throw 'cargo was not found - install Rust from https://rustup.rs' }
# The GNU linker cannot read paths with non-Latin letters: keep build files in a Latin-only folder in that case.
if (-not $env:CARGO_TARGET_DIR -and $root -match '[^\x00-\x7F]') { $env:CARGO_TARGET_DIR = Join-Path $env:SystemDrive '\rust-build\ReinstallKit' }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }

Push-Location $root
try { & $cargo build --release; if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" } } finally { Pop-Location }

$out = Join-Path $root 'dist\ReinstallKit'
New-Item -ItemType Directory -Force -Path $out, (Join-Path $out 'licenses') | Out-Null
$exe = Join-Path $out 'ReinstallKit.exe'
Copy-Item -LiteralPath (Join-Path $target 'release\ReinstallKit.exe') -Destination $exe -Force
& (Join-Path $target 'release\stamp.exe') $exe (Join-Path $root 'assets\ReinstallKit.ico')
if ($LASTEXITCODE -ne 0) { throw 'could not write the icon into the exe' }
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $out 'licenses\ReinstallKit-LICENSE.txt') -Force
'{0}  {1:N1} MB' -f $exe, ((Get-Item -LiteralPath $exe).Length / 1MB)

if ($Installer) {
    $iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
    if (-not $iscc) {
        $loc = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
            Where-Object { $_.DisplayName -like 'Inno Setup*' -and $_.InstallLocation } | Select-Object -First 1 -ExpandProperty InstallLocation
        if ($loc -and (Test-Path -LiteralPath (Join-Path $loc 'ISCC.exe'))) { $iscc = Join-Path $loc 'ISCC.exe' }
    }
    if (-not $iscc) { throw 'Inno Setup 6 was not found (https://jrsoftware.org/isinfo.php)' }
    & $iscc /Q (Join-Path $root 'installer\ReinstallKit.iss')
    if ($LASTEXITCODE -ne 0) { throw "installer build failed ($LASTEXITCODE)" }
    $setup = Join-Path $root 'dist\ReinstallKit-setup.exe'
    '{0}  {1:N1} MB' -f $setup, ((Get-Item -LiteralPath $setup).Length / 1MB)
}
