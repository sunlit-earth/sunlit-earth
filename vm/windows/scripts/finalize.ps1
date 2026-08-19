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

# The job task has to run in the console session. A task that ended up with
# any other logon type would run invisibly and every windowed test would fail
# in a way that looks like the product's fault.
$principal = (Get-ScheduledTask -TaskName 'sunlit-e2e-job').Principal
if ($principal.LogonType -ne 'Interactive') {
    $problems += "the job task has logon type $($principal.LogonType), not Interactive"
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
