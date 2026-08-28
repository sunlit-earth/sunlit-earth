# Last pass over the Windows builder layer: prove every part of the toolchain is
# there and findable, and say where each of them is.
#
# The verification is the point of this being a script rather than a comment. A
# layer missing any one of these boots, answers SSH, accepts a source archive,
# and fails somewhere in a forty-minute compile: no `rc.exe` fails in
# `embed-resource`, no libclang fails in bindgen, no `dumpbin` fails at the end
# when the host asks what the binary imports. Each of those is expensive to
# diagnose from the other side of a guest boundary, and each is free to check
# here while there is still a build to fail.

param(
    # The channel `toolchain.ps1` installed, checked by name rather than as the
    # default: the build job calls it by name too.
    [Parameter(Mandatory = $true)]
    [string]$Channel
)

$ErrorActionPreference = 'Stop'

$problems = @()
$libclang = 'C:\tools\llvm\bin\libclang.dll'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'

Write-Output '== the Rust toolchain'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$rustc = Join-Path $env:USERPROFILE '.cargo\bin\rustc.exe'
foreach ($exe in @($cargo, $rustc)) {
    if (-not (Test-Path $exe)) { $problems += "$exe is missing" }
}
if (Test-Path $rustc) {
    # By name, because that is how the build job asks for it. A toolchain that
    # answers only as the default would fail there instead of here.
    $version = & $rustc "+$Channel" -vV 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $problems += "rustc +$Channel does not answer: $version"
    } else {
        Write-Output ($version.Trim())
    }
}
if (Test-Path $cargo) {
    $cargoVersion = & $cargo "+$Channel" -V 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $problems += "cargo +$Channel does not answer: $cargoVersion"
    } else {
        Write-Output ($cargoVersion.Trim())
    }
}

Write-Output '== the MSVC toolset'
if (-not (Test-Path $vswhere)) {
    $problems += 'vswhere.exe is missing, so nothing can find the MSVC toolset'
} else {
    # The component rather than a path: where the toolset lands moves between
    # installer versions, and this is the question the `cc` crate and rustc's own
    # linker lookup ask.
    $install = & $vswhere -latest -products '*' `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>&1 | Out-String
    $install = $install.Trim()
    if (-not $install) {
        $problems += 'vswhere found no installation with the VC tools component'
    } else {
        Write-Output "VC tools in $install"

        # `rc.exe` comes from the Windows SDK, not from the toolset, and it is
        # what `embed-resource` needs for the app's icon. It sits under a
        # version-numbered directory, so it is searched for rather than named.
        $sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
        $rc = Get-ChildItem -Path $sdk -Filter 'rc.exe' -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like '*\x64\rc.exe' } |
            Select-Object -First 1
        if (-not $rc) {
            $problems += "no x64 rc.exe under $sdk, so embed-resource cannot compile the icon"
        } else {
            Write-Output "rc.exe at $($rc.FullName)"
        }

        # `dumpbin.exe` is how the build job reports what the binary imports,
        # which is what proves the static C runtime on the artifact itself.
        $dumpbin = Get-ChildItem -Path (Join-Path $install 'VC\Tools\MSVC') `
            -Filter 'dumpbin.exe' -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like '*\Hostx64\x64\dumpbin.exe' } |
            Select-Object -First 1
        if (-not $dumpbin) {
            $problems += 'no x64 dumpbin.exe under the toolset, so a build could not report its own imports'
        } else {
            Write-Output "dumpbin.exe at $($dumpbin.FullName)"
        }
    }
}

Write-Output '== libclang for bindgen'
if (-not (Test-Path $libclang)) {
    $problems += "$libclang is missing, so bindgen cannot run and the astronomy bindings cannot build"
} else {
    Write-Output "libclang.dll at $libclang"
}

if ($problems.Count -gt 0) {
    Write-Output ''
    Write-Output 'this layer is not usable:'
    foreach ($problem in $problems) { Write-Output "  - $problem" }
    throw "$($problems.Count) problem(s) with the toolchain in this layer"
}

Write-Output ''
Write-Output 'the toolchain is complete'
