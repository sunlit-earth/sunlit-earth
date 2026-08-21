# Last pass over the Windows golden image, run by Packer over SSH.
#
# Two jobs: check that the bootstrap actually did what it claimed, and leave
# the image in the state every overlay should boot from.

$ErrorActionPreference = 'Stop'
$root = 'C:\sunlit-e2e'

Write-Output '== verifying the bootstrap'
$problems = @()

if ((Get-Service sshd -ErrorAction SilentlyContinue).Status -ne 'Running') {
    $problems += 'the sshd service is not running'
}
foreach ($task in @('sunlit-e2e-job', 'sunlit-e2e-session-ready')) {
    if (-not (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue)) {
        $problems += "the $task scheduled task is missing"
    }
}
foreach ($path in @("$root\run-job.cmd", "$root\session-ready.cmd", "$root\bin", "$root\results")) {
    if (-not (Test-Path $path)) { $problems += "$path is missing" }
}

# Windows ships neither of these and every Rust MSVC binary the suite runs links
# them dynamically, so an image without them boots fine and then answers every
# test with exit code 0xC0000135 and an empty log. Checked here because that is
# the one failure the image cannot report about itself later.
foreach ($dll in @('vcruntime140.dll', 'msvcp140.dll')) {
    if (-not (Test-Path (Join-Path $env:SystemRoot "System32\$dll"))) {
        $problems += "$dll is missing, so the binaries this image exists to run cannot start"
    }
}

# The job task has to run in the console session. A task that ended up with
# any other logon type would run invisibly and every windowed test would fail
# in a way that looks like the product's fault.
#
# Looked up with -ErrorAction SilentlyContinue: without it, the missing-task
# case that the loop above is here to report would instead throw out of this
# script and lose every other problem with it.
$job = Get-ScheduledTask -TaskName 'sunlit-e2e-job' -ErrorAction SilentlyContinue
if ($job -and $job.Principal.LogonType -ne 'Interactive') {
    $problems += "the job task has logon type $($job.Principal.LogonType), not Interactive"
}

# What makes this image's desktop reachable without a password, and a test run
# in it safe from being taken over: with Remote Desktop Services enabled,
# vmconnect offers an enhanced session, asks for credentials, and moves the
# console session into an RDP one. A guest handed to a person turns it back on
# for itself. Checked here because the service cannot be stopped once it is
# running, so the start type is the only evidence available before the shutdown
# that ends the build.
if ((Get-Service TermService).StartType -ne 'Disabled') {
    $problems += 'Remote Desktop Services is not disabled, so vmconnect would offer an enhanced session'
}

Write-Output '== ensuring a boot path that survives the hypervisor change'
# This image is installed under OVMF and then booted on Hyper-V generation 2.
# The boot entry Windows Setup wrote lives in OVMF's own NVMe-backed variable
# store, which Hyper-V has no equivalent of and never sees, and
# `Set-VMFirmware -FirstBootDevice` names a disk rather than a loader. What
# both firmwares agree on is the removable-media fallback path: if
# \EFI\Boot\bootx64.efi exists on the EFI system partition, UEFI will boot it
# without any variable at all.
#
# So the ESP gets an explicit bcdboot pass and, if Setup did not leave the
# fallback loader behind, a copy of the boot manager under that name. Neither
# step can be exercised without a real build; this is the standard mitigation
# and Deviation 16 in the phase 3 plan records it and its unverified status.
# A free letter, not a hardcoded S:. `Test-Path 'S:'` answers "something is
# mounted at S:", which is not the same as "S: is the EFI system partition",
# and on a machine where it is a data drive this would have run bcdboot
# against it.
$esp = $null
foreach ($letter in @('S', 'T', 'U', 'V', 'W', 'X', 'Y')) {
    if (-not (Test-Path "${letter}:\")) {
        $esp = $letter
        break
    }
}

if (-not $esp) {
    $problems += 'no free drive letter to mount the EFI system partition on'
} else {
    $espMounted = $false
    try {
        # mountvol and bcdboot are native executables: they do not throw, so
        # $ErrorActionPreference does nothing for them and the exit code is the
        # only thing that reports a failure.
        mountvol "${esp}:" /S
        if ($LASTEXITCODE -ne 0) {
            $problems += "mountvol ${esp}: /S failed with exit code $LASTEXITCODE"
        } else {
            $espMounted = $true
            bcdboot C:\Windows /s "${esp}:" /f UEFI | Out-Null
            if ($LASTEXITCODE -ne 0) {
                $problems += "bcdboot failed with exit code $LASTEXITCODE"
            }

            $fallbackDir = "${esp}:\EFI\Boot"
            $fallback = Join-Path $fallbackDir 'bootx64.efi'
            $bootManager = "${esp}:\EFI\Microsoft\Boot\bootmgfw.efi"
            if (-not (Test-Path $fallback)) {
                if (Test-Path $bootManager) {
                    New-Item -ItemType Directory -Force -Path $fallbackDir | Out-Null
                    Copy-Item $bootManager $fallback -Force
                    Write-Output "copied the boot manager to $fallback"
                } else {
                    $problems += 'no boot manager on the EFI system partition to fall back to'
                }
            }
            if (-not (Test-Path $fallback)) {
                $problems += 'no fallback loader at EFI\Boot\bootx64.efi, so this image would not boot'
            }
        }
    } catch {
        $problems += "could not prepare the EFI system partition: $_"
    } finally {
        # Only if it was actually mounted here.
        if ($espMounted) { mountvol "${esp}:" /D }
    }
}

if ($problems.Count -gt 0) {
    Write-Output 'the image is not usable:'
    $problems | ForEach-Object { Write-Output "  $_" }
    exit 1
}

Write-Output '== clearing per-run state'
# The marker is written at logon. Shipping one inside the image would make the
# orchestrator believe a desktop exists before the guest has finished booting.
Remove-Item "$root\ready" -Force -ErrorAction SilentlyContinue
Remove-Item "$root\job.cmd" -Force -ErrorAction SilentlyContinue
Remove-Item "$root\results" -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$root\results\artifacts" | Out-Null

Write-Output '== trimming'
Remove-Item "$env:TEMP\*" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item 'C:\Windows\Temp\*' -Recurse -Force -ErrorAction SilentlyContinue

Write-Output 'the image is ready'
