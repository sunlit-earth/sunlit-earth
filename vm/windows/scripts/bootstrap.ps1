# First-logon bootstrap for the Windows golden image.
#
# Run once, by FirstLogonCommands in the unattend file, from the CD the Packer
# build attaches. Not a floppy: the q35 machine this is built on has no floppy
# controller. Everything it sets up has to survive into every
# throwaway overlay booted from the finished image.
#
# It is deliberately noisy: its transcript is the only record of what happened
# between "Windows installed" and "Packer connected over SSH", and if SSH never
# comes up that transcript is the only place to look.

$ErrorActionPreference = 'Stop'
$root = 'C:\sunlit-e2e'
New-Item -ItemType Directory -Force -Path $root | Out-Null
Start-Transcript -Path "$root\bootstrap.log" -Append | Out-Null

function Step($name) { Write-Output "== $name" }

try {
    Step 'directories'
    foreach ($dir in @("$root\bin", "$root\results", "$root\results\artifacts", "$root\fixtures")) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    # The media is a CD, whose drive letter is not fixed. $PSScriptRoot is
    # wherever this script was started from, and everything the bootstrap needs
    # sits beside it, so nothing has to guess a letter.
    $media = $PSScriptRoot
    Write-Output "media: $media"
    Copy-Item (Join-Path $media 'run-job.cmd') "$root\run-job.cmd" -Force
    Copy-Item (Join-Path $media 'session-ready.cmd') "$root\session-ready.cmd" -Force

    Step 'openssh server'
    # The capability is present in the image and only has to be turned on, so
    # this needs no network. It is the one part of the bootstrap that cannot be
    # retried later: without it there is no way in.
    Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1.0' | Out-Null
    Set-Service -Name sshd -StartupType Automatic
    Start-Service sshd

    Step 'authorized key'
    # sshd ignores an administrator's own authorized_keys file and reads this
    # one instead, and it refuses to read it at all unless the ACL is limited
    # to Administrators and SYSTEM.
    $adminKeys = "$env:ProgramData\ssh\administrators_authorized_keys"
    Copy-Item (Join-Path $media 'authorized_keys') $adminKeys -Force
    icacls $adminKeys /inheritance:r | Out-Null
    icacls $adminKeys /grant 'Administrators:F' | Out-Null
    icacls $adminKeys /grant 'SYSTEM:F' | Out-Null

    Step 'visual c++ runtime'
    # Windows ships no vcruntime140.dll, and every Rust MSVC binary this suite
    # runs links it dynamically. Without it the test harness cannot start at all,
    # and the only symptom is exit code 0xC0000135 with an empty output.log:
    # STATUS_DLL_NOT_FOUND, which prints nothing anywhere. Measured on
    # 2026-08-20 on a fresh build 26100: System32 has only the _clr0400
    # variants, which belong to .NET and do not satisfy the loader.
    #
    # This is the one thing either image build fetches from inside the guest,
    # and it is also exactly what a user installing the product needs, so a
    # guest that has it is a guest that looks like the machine under test.
    # `finalize.ps1` checks for the DLLs afterwards, so a guest with no network
    # fails the build here rather than producing an image whose binaries cannot
    # run.
    $redist = Join-Path $env:TEMP 'vc_redist.x64.exe'
    curl.exe -L -o $redist 'https://aka.ms/vs/17/release/vc_redist.x64.exe'
    if ($LASTEXITCODE -ne 0) {
        throw "could not download the Visual C++ runtime: curl exited $LASTEXITCODE"
    }
    # A native installer: it does not throw, and 3010 means "installed, wants a
    # reboot", which is not a failure for an image that is about to be shut down.
    & $redist /install /quiet /norestart | Out-Null
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 3010) {
        throw "the Visual C++ runtime installer exited $LASTEXITCODE"
    }
    Remove-Item $redist -Force -ErrorAction SilentlyContinue

    Step 'firewall'
    if (-not (Get-NetFirewallRule -Name 'sunlit-e2e-ssh' -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -Name 'sunlit-e2e-ssh' -DisplayName 'sunlit-e2e SSH' `
            -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 | Out-Null
    }

    Step 'remote sessions'
    # This image boots offering no remote session, and that is the safe default
    # rather than the final word. With Remote Desktop Services running, Hyper-V
    # reports enhanced session mode as available and vmconnect switches to it by
    # itself; an enhanced session is RDP, and connecting takes the console
    # session over, which is where a windowed test run keeps its desktop. A guest
    # with a job in it must not offer that.
    #
    # A guest handed to a person is a different matter, and there the enhanced
    # session is the only one that can be resized. So `vm up` and `e2e --keep`
    # turn this back on per guest (`guest::handover::enable_enhanced_session`),
    # along with blanking the account's password so the dialog can be dismissed
    # rather than filled in. Starting the service at runtime works, which is what
    # makes that possible without a rebuild.
    #
    # Disabled rather than stopped, because the service refuses to stop once it
    # is running: the boot that matters is the next one, and every boot of the
    # finished image is a next one. Measured on 2026-08-21: disabled, the host
    # reports EnhancedSessionModeState 6, "allowed but not available", and
    # vmconnect can only open a basic session, which shows the console desktop
    # the autologon has already signed into and asks for nothing. Started again,
    # the host reports 2 within seconds. The console session, the scheduled tasks
    # that run in it and `qwinsta` are unaffected either way.
    Set-Service -Name TermService -StartupType Disabled

    Step 'power and display'
    # Nothing may sleep, blank, or lock under a long test run.
    powercfg /change standby-timeout-ac 0
    powercfg /change monitor-timeout-ac 0
    powercfg /change hibernate-timeout-ac 0
    powercfg /change disk-timeout-ac 0
    reg add 'HKCU\Control Panel\Desktop' /v ScreenSaveActive /t REG_SZ /d 0 /f | Out-Null
    reg add 'HKLM\SOFTWARE\Policies\Microsoft\Windows\Personalization' /v NoLockScreen /t REG_DWORD /d 1 /f | Out-Null

    Step 'noise'
    # A first-run animation, a "let us finish setting up" page, or a search
    # highlight all steal focus from whatever a test just opened.
    reg add 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' /v EnableFirstLogonAnimation /t REG_DWORD /d 0 /f | Out-Null
    reg add 'HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\ContentDeliveryManager' /v SubscribedContent-310093Enabled /t REG_DWORD /d 0 /f | Out-Null
    reg add 'HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\ContentDeliveryManager' /v SubscribedContent-338389Enabled /t REG_DWORD /d 0 /f | Out-Null
    reg add 'HKLM\SOFTWARE\Policies\Microsoft\Windows\CloudContent' /v DisableWindowsConsumerFeatures /t REG_DWORD /d 1 /f | Out-Null
    reg add 'HKLM\SOFTWARE\Policies\Microsoft\Windows\OOBE' /v DisablePrivacyExperience /t REG_DWORD /d 1 /f | Out-Null
    # Windows Update rebooting mid-run would look exactly like a flaky test.
    reg add 'HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU' /v NoAutoUpdate /t REG_DWORD /d 1 /f | Out-Null

    Step 'job task'
    # "Run only when the user is logged on" is the whole point: a task with
    # that principal executes in the console session, which is the one place a
    # window can appear. A process started over SSH cannot get there.
    $action = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/c `"$root\run-job.cmd`""
    $principal = New-ScheduledTaskPrincipal -UserId 'tester' -LogonType Interactive -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit (New-TimeSpan -Hours 3) -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskName 'sunlit-e2e-job' -Action $action -Principal $principal `
        -Settings $settings -Force | Out-Null

    Step 'session marker task'
    # Written at every logon, which is what tells the orchestrator a desktop
    # exists. The golden image ships without the marker file, and every run
    # boots a fresh overlay, so a stale one cannot be left behind.
    $readyAction = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/c `"$root\session-ready.cmd`""
    $readyTrigger = New-ScheduledTaskTrigger -AtLogOn -User 'tester'
    Register-ScheduledTask -TaskName 'sunlit-e2e-session-ready' -Action $readyAction `
        -Trigger $readyTrigger -Principal $principal -Force | Out-Null

    Step 'done'
    Write-Output 'bootstrap complete'
}
catch {
    Write-Output "bootstrap failed: $_"
    Write-Output $_.ScriptStackTrace
    throw
}
finally {
    Stop-Transcript | Out-Null
}
