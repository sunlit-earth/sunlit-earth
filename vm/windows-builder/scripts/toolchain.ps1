# What makes the Windows layer a builder: the MSVC toolset, a Windows SDK,
# libclang for bindgen, and rustup with the pinned toolchain. Run by
# `cargo xtask vm build-image windows-builder` over SSH, inside a guest booted
# from a differencing child of the Windows desktop image.
#
# Everything below this script is inherited from the parent image: the account,
# the SSH server, the scheduled job task, the session marker, the autologon and
# the firewall rule. So nothing here touches any of that, and a mistake here
# costs twenty minutes rather than the hour a Windows install takes.
#
# A Windows OpenSSH session for a member of Administrators carries the full
# token, so the installers below run elevated without anything being done about
# it. That is worth knowing rather than discovering: the same script run from an
# unelevated shell would fail in the middle of the Visual Studio installer.

param(
    # The rustup channel to install, which the xtask reads out of
    # `rust-toolchain.toml`. A release build installs it again by name in its own
    # job, so this is a warm cache rather than the contract.
    [Parameter(Mandatory = $true)]
    [string]$Channel
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$tools = 'C:\tools'
# Spelled out rather than joined, because the build job points
# LIBCLANG_PATH at the same string from a constant on the host and a test
# compares the two.
$llvm = 'C:\tools\llvm'
$llvmBin = 'C:\tools\llvm\bin'
# The major version `ci.yml` pins on the Windows runner. bindgen needs a
# libclang, not a clang driver, and this is the one the workspace is known to
# build through.
$llvmVersion = '19.1.7'
$llvmArchive = "clang+llvm-$llvmVersion-x86_64-pc-windows-msvc.tar.xz"
$llvmUrl = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$llvmVersion/$llvmArchive"

$buildToolsUrl = 'https://aka.ms/vs/17/release/vs_BuildTools.exe'
# The two components a Rust build uses, rather than the workload that contains
# them and several gigabytes of everything else. The SDK is what brings `rc.exe`,
# which `embed-resource` requires for the app's icon resource and which
# `manifest_required()` fails the build without.
$vsComponents = @(
    'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    'Microsoft.VisualStudio.Component.Windows11SDK.22621'
)

New-Item -ItemType Directory -Force -Path $tools | Out-Null

Write-Output '== excluding the build directories from Defender'
# Scanning every object file a release build writes is a measurable tax, and this
# guest is a throwaway with no route off the host. Failures are tolerated: on a
# guest where Defender is managed by policy the exclusion is refused, and a
# slower build is better than a failed one.
$exclusions = @(
    'C:\sunlit-e2e',
    "$env:USERPROFILE\.cargo",
    "$env:USERPROFILE\.rustup",
    $tools
)
foreach ($path in $exclusions) {
    try {
        Add-MpPreference -ExclusionPath $path -ErrorAction Stop
        Write-Output "   excluded $path"
    } catch {
        Write-Output "   could not exclude $path ($($_.Exception.Message)); carrying on"
    }
}

Write-Output '== installing the Visual Studio Build Tools'
Write-Output '   this is several gigabytes and says nothing for ten to twenty minutes'
$bootstrapper = Join-Path $env:TEMP 'vs_BuildTools.exe'
Invoke-WebRequest -Uri $buildToolsUrl -OutFile $bootstrapper -UseBasicParsing
$vsArgs = @('--quiet', '--wait', '--norestart', '--nocache')
foreach ($component in $vsComponents) {
    $vsArgs += '--add'
    $vsArgs += $component
}
$vs = Start-Process -FilePath $bootstrapper -ArgumentList $vsArgs -Wait -PassThru
# 3010 is "installed, needs a reboot", which the shutdown that ends this build
# absorbs. Everything else is a failure.
if ($vs.ExitCode -ne 0 -and $vs.ExitCode -ne 3010) {
    throw "the Visual Studio Build Tools installer exited $($vs.ExitCode)"
}
Write-Output "   the installer exited $($vs.ExitCode)"
Remove-Item $bootstrapper -Force -ErrorAction SilentlyContinue

Write-Output '== extracting libclang for bindgen'
# The release archive rather than the LLVM installer, which is two gigabytes and
# brings a whole toolchain nothing here compiles with. Windows 11 ships bsdtar as
# `tar.exe`, which reads .tar.xz, so there is nothing to install to unpack it.
$archive = Join-Path $env:TEMP $llvmArchive
Invoke-WebRequest -Uri $llvmUrl -OutFile $archive -UseBasicParsing
$staging = Join-Path $env:TEMP 'llvm-staging'
Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $staging | Out-Null
tar.exe -xf $archive -C $staging
if ($LASTEXITCODE -ne 0) { throw "tar.exe exited $LASTEXITCODE unpacking $llvmArchive" }
$unpacked = Get-ChildItem -Path $staging -Directory | Select-Object -First 1
if (-not $unpacked) { throw "$llvmArchive unpacked to nothing" }
Remove-Item $llvm -Recurse -Force -ErrorAction SilentlyContinue
# Only what bindgen loads: the DLL and the headers clang resolves `#include`
# against. The rest of the archive is a compiler this image does not use.
New-Item -ItemType Directory -Force -Path $llvmBin | Out-Null
Copy-Item -Path (Join-Path $unpacked.FullName 'bin\libclang.dll') `
    -Destination (Join-Path $llvmBin 'libclang.dll') -Force
Copy-Item -Path (Join-Path $unpacked.FullName 'lib\clang') `
    -Destination (Join-Path $llvm 'lib\clang') -Recurse -Force
Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $archive -Force -ErrorAction SilentlyContinue
Write-Output "   libclang.dll is in $llvmBin"

Write-Output "== installing the $Channel toolchain with rustup"
$rustupInit = Join-Path $env:TEMP 'rustup-init.exe'
Invoke-WebRequest -Uri 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' `
    -OutFile $rustupInit -UseBasicParsing
# As the account this session is, which is the account the job task runs as and
# therefore the account whose `.cargo\bin\cargo.exe` the build job names.
& $rustupInit -y --default-toolchain $Channel --profile minimal
if ($LASTEXITCODE -ne 0) { throw "rustup-init exited $LASTEXITCODE" }
Remove-Item $rustupInit -Force -ErrorAction SilentlyContinue

Write-Output '== done'
