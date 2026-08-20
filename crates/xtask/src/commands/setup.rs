//! `cargo xtask vm setup`: the one command that changes the host.
//!
//! Plan decision 3. Elevated on Windows, unelevated with targeted `sudo` on
//! Linux, idempotent, and it never reboots: it reports what needs a restart or
//! a relogin and hands back to `vm doctor` for confirmation. Deciding what to
//! do is a pure function of the host facts, so "running it twice is a no-op the
//! second time" is a unit test rather than a claim.

use std::fmt::Write as _;

use crate::host::facts::{
    FEATURE_HYPERV, FEATURE_WHPX, FeatureState, HYPERV_ADMINS_SID, HostFacts, WINDOWS_QEMU_DIRS,
    WSL_DISTRO,
};
use crate::provider::target::HostOs;
use crate::runner::{Cmd, Runner, powershell, ps_quote};
use crate::store::Store;

/// The winget package identifiers, verified against the community repository on
/// 2026-08-19. Note the lowercase `c` in `Hashicorp`: the manifest folder is
/// `manifests/h/Hashicorp/Packer` even though the publisher writes it `HashiCorp`.
pub const WINGET_QEMU: &str = "SoftwareFreedomConservancy.QEMU";
pub const WINGET_PACKER: &str = "Hashicorp.Packer";

/// The ISO builder, for Packer's `cd_content`.
///
/// Verified against the live winget source on 2026-08-20: a portable package,
/// one 140 KB executable pulled hash-pinned from Microsoft's public symbol
/// server, declaring `oscdimg` as its command so winget links it onto `PATH`.
/// That is the tool Packer looks for last but the only one of its four with a
/// one-command install on Windows: there is no winget package for xorriso or
/// mkisofs, `hdiutil` is macOS only, and the other route to `oscdimg` is the
/// Windows ADK, which is several gigabytes for the same binary and does not
/// put it on `PATH` afterwards.
///
/// Two caveats worth knowing rather than discovering. The manifest is a
/// community submission rather than a Microsoft-published package, and the
/// binary is proprietary with no clear standalone redistribution grant, so it
/// is reasonable for a developer to install and not something to vendor. On
/// Linux the equivalent is xorriso, which every distribution packages.
pub const WINGET_OSCDIMG: &str = "Microsoft.OSCDIMG";

/// The script one winget step runs.
///
/// It asks before it installs, for two reasons. Detection reads this process's
/// `PATH`, which cannot see the links directory winget appended during an
/// earlier run in the same shell, so a step can be planned for a package that
/// is already there. And `winget install` on a package it finds installed does
/// not exit zero: it tries an upgrade, finds none, and fails, which a second
/// `vm setup` in the same shell then reported as two broken steps.
///
/// Which code winget used is not recoverable from here, because
/// `powershell.exe -EncodedCommand` collapses a native command's exit code to
/// 1, so the script decides for itself and says which case it was.
pub fn winget_install_script(id: &str) -> String {
    format!(
        "$id = '{id}'\n\
         winget list --id $id --exact --accept-source-agreements 2>$null | Out-Null\n\
         if ($LASTEXITCODE -eq 0) {{\n  \
         Write-Output \"$id is already installed\"\n  \
         exit 0\n\
         }}\n\
         winget install --id $id --exact --silent \
         --accept-package-agreements --accept-source-agreements\n\
         $code = $LASTEXITCODE\n\
         if ($code -ne 0) {{\n  \
         winget list --id $id --exact 2>$null | Out-Null\n  \
         if ($LASTEXITCODE -eq 0) {{\n    \
         Write-Output \"winget exited $code, but $id is installed\"\n    \
         exit 0\n  \
         }}\n  \
         Write-Output \"winget install $id failed with exit code $code\"\n  \
         exit 1\n\
         }}"
    )
}

/// Build dependencies for the Linux guest's binaries, matching the list in
/// CLAUDE.md. `mesa-vulkan-drivers` supplies lavapipe, which is the adapter the
/// GPU tests use.
pub const LINUX_BUILD_DEPS: &str = "build-essential pkg-config clang libclang-dev \
     libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
     mesa-vulkan-drivers xvfb";

/// One thing setup may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: &'static str,
    /// The script to run, in the host's shell.
    pub script: String,
    /// False when the host is already in the desired state.
    pub needed: bool,
    /// Why it is needed, or why it was skipped.
    pub note: String,
    /// Whether finishing this step can leave a restart pending.
    pub may_need_restart: bool,
    /// Whether its effect only reaches a new logon session.
    pub needs_relogin: bool,
}

impl Step {
    fn new(name: &'static str, script: String, needed: bool, note: impl Into<String>) -> Self {
        Self {
            name,
            script,
            needed,
            note: note.into(),
            may_need_restart: false,
            needs_relogin: false,
        }
    }

    #[must_use]
    fn restart(mut self) -> Self {
        self.may_need_restart = true;
        self
    }

    #[must_use]
    fn relogin(mut self) -> Self {
        self.needs_relogin = true;
        self
    }
}

/// What setup knows before it starts.
#[derive(Debug, Clone, Default)]
pub struct SetupInputs {
    pub facts: HostFacts,
    /// Whether the WSL distribution already has the toolchain and the build
    /// dependencies. Probed by the setup command, because asking costs a distro
    /// start and the doctor is not allowed to change anything.
    pub wsl_ready: bool,
    pub ssh_key_present: bool,
}

/// The Windows plan.
pub fn windows_plan(inputs: &SetupInputs, store: &Store) -> Vec<Step> {
    let mut steps = windows_feature_steps(inputs);
    steps.extend(windows_tool_steps(inputs));
    steps.extend(windows_access_steps(inputs));
    steps.push(ssh_key_step(inputs, store, HostOs::Windows));
    steps
}

/// The two optional features, enabled without restarting.
fn windows_feature_steps(inputs: &SetupInputs) -> Vec<Step> {
    let windows = inputs.facts.windows.clone().unwrap_or_default();
    let mut steps = Vec::new();
    for (name, feature, why) in [
        (
            "hyper-v feature",
            FEATURE_HYPERV,
            "the Windows guest runs on Hyper-V",
        ),
        (
            "hypervisor platform",
            FEATURE_WHPX,
            "QEMU's WHPX acceleration runs on top of the Hyper-V hypervisor",
        ),
    ] {
        let state = windows.feature(feature);
        steps.push(
            Step::new(
                name,
                format!(
                    "$result = Enable-WindowsOptionalFeature -Online -FeatureName {feature} \
                     -All -NoRestart\nWrite-Output \"RESTART_NEEDED=$($result.RestartNeeded)\""
                ),
                state != FeatureState::Enabled,
                if state == FeatureState::Enabled {
                    "already enabled".to_owned()
                } else {
                    why.to_owned()
                },
            )
            .restart(),
        );
    }
    steps
}

/// QEMU and Packer, including the `PATH` entry the QEMU installer omits.
fn windows_tool_steps(inputs: &SetupInputs) -> Vec<Step> {
    let facts = &inputs.facts;
    let windows = facts.windows.clone().unwrap_or_default();
    let mut steps = Vec::new();

    let qemu_present = facts.tool("qemu-system-x86_64").is_some();
    steps.push(Step::new(
        "qemu",
        winget_install_script(WINGET_QEMU),
        !qemu_present,
        if qemu_present {
            "already installed".to_owned()
        } else {
            format!("winget package {WINGET_QEMU}")
        },
    ));

    // The QEMU installer writes no environment key at all, so a perfectly good
    // install stays invisible to every tool that looks on PATH.
    let qemu_dir = WINDOWS_QEMU_DIRS[0];
    let path_ok = windows.path_contains(qemu_dir);
    steps.push(
        Step::new(
            "qemu on PATH",
            format!(
                "$dir = {dir}\n\
                 $current = [Environment]::GetEnvironmentVariable('Path','Machine')\n\
                 if ($current -split ';' -notcontains $dir) {{\n  \
                 [Environment]::SetEnvironmentVariable('Path', ($current.TrimEnd(';') + ';' + $dir), 'Machine')\n\
                 }}",
                dir = ps_quote(qemu_dir)
            ),
            !path_ok,
            if path_ok {
                format!("{qemu_dir} is already on PATH")
            } else {
                format!("the QEMU installer does not put {qemu_dir} on PATH")
            },
        )
        .relogin(),
    );

    // Packer shells out to build the small CD each template hands its guest,
    // and fails at once if it cannot find a tool for it.
    // Whether it is installed is the question, not whether this shell can see
    // it yet: a second copy of a tool winget already has is not an improvement.
    let iso_present = inputs.facts.iso_tool_installed().is_some();
    steps.push(Step::new(
        "iso builder",
        winget_install_script(WINGET_OSCDIMG),
        !iso_present,
        if iso_present {
            "already installed".to_owned()
        } else {
            format!("winget package {WINGET_OSCDIMG}, a 140 KB portable oscdimg")
        },
    ));

    let packer_present = facts.tool("packer").is_some();
    steps.push(Step::new(
        "packer",
        winget_install_script(WINGET_PACKER),
        !packer_present,
        if packer_present {
            "already installed".to_owned()
        } else {
            format!("winget package {WINGET_PACKER}, which does land on PATH")
        },
    ));
    steps
}

/// Group membership and the WSL distribution, the two things whose effect
/// arrives in a later session rather than immediately.
fn windows_access_steps(inputs: &SetupInputs) -> Vec<Step> {
    let windows = inputs.facts.windows.clone().unwrap_or_default();
    let mut steps = Vec::new();

    steps.push(
        Step::new(
            "hyper-v administrators",
            format!(
                "$group = Get-LocalGroup -SID '{HYPERV_ADMINS_SID}'\n\
                 $user = [Security.Principal.WindowsIdentity]::GetCurrent().Name\n\
                 if (-not (Get-LocalGroupMember -Group $group -ErrorAction SilentlyContinue | \
                 Where-Object {{ $_.Name -eq $user }})) {{\n  \
                 Add-LocalGroupMember -Group $group -Member $user\n}}"
            ),
            !windows.in_hyperv_admins,
            if windows.in_hyperv_admins {
                "already a member".to_owned()
            } else {
                "membership is what lets e2e, vm status, and vm destroy drive Hyper-V \
                 without elevation"
                    .to_owned()
            },
        )
        .relogin(),
    );

    // `--no-launch` registers the application but not the distribution for an
    // Appx-based distro such as Ubuntu-22.04, so the launcher still has to run
    // once; `install --root` is the documented way to do that without the
    // interactive account prompt (microsoft/WSL issues 10646 and 10386,
    // microsoft/WSL-DistroLauncher).
    let distro_present = windows.distro(WSL_DISTRO).is_some();
    steps.push(Step::new(
        "wsl distro",
        format!(
            "wsl.exe --install -d {WSL_DISTRO} --no-launch\n\
             ubuntu2204.exe install --root"
        ),
        !distro_present,
        if distro_present {
            format!("{WSL_DISTRO} is already registered")
        } else {
            format!("{WSL_DISTRO} builds the Linux guest's binaries, and matches its glibc")
        },
    ));

    steps.push(Step::new(
        "wsl build dependencies",
        format!(
            "wsl.exe -d {WSL_DISTRO} --user root -- bash -lc {script}",
            script = ps_quote(&wsl_provision_script())
        ),
        !inputs.wsl_ready,
        if inputs.wsl_ready {
            "the toolchain and build dependencies are present".to_owned()
        } else {
            "the Rust toolchain and the build dependencies from CLAUDE.md".to_owned()
        },
    ));

    steps
}

/// The Linux plan.
pub fn linux_plan(inputs: &SetupInputs, store: &Store) -> Vec<Step> {
    let facts = &inputs.facts;
    let linux = facts.linux.clone().unwrap_or_default();
    let mut steps = Vec::new();

    let qemu_present =
        facts.tool("qemu-system-x86_64").is_some() && facts.tool("qemu-img").is_some();
    let iso_present = inputs.facts.iso_tool_installed().is_some();
    steps.push(Step::new(
        "iso builder",
        "sudo apt-get update && sudo apt-get install -y xorriso".to_owned(),
        !iso_present,
        if iso_present {
            "already installed".to_owned()
        } else {
            "Packer shells out to xorriso to build the CD each template hands its guest".to_owned()
        },
    ));

    steps.push(Step::new(
        "qemu",
        "sudo apt-get update && sudo apt-get install -y qemu-system-x86 qemu-utils".to_owned(),
        !qemu_present,
        if qemu_present {
            "already installed".to_owned()
        } else {
            "qemu-system-x86 and qemu-utils".to_owned()
        },
    ));

    // Packer is not in the Ubuntu archive; this is HashiCorp's documented apt
    // repository, added explicitly rather than silently.
    let packer_present = facts.tool("packer").is_some();
    steps.push(Step::new(
        "packer",
        "wget -qO- https://apt.releases.hashicorp.com/gpg | \
         sudo gpg --dearmor -o /usr/share/keyrings/hashicorp-archive-keyring.gpg && \
         echo \"deb [signed-by=/usr/share/keyrings/hashicorp-archive-keyring.gpg] \
         https://apt.releases.hashicorp.com $(lsb_release -cs) main\" | \
         sudo tee /etc/apt/sources.list.d/hashicorp.list && \
         sudo apt-get update && sudo apt-get install -y packer"
            .to_owned(),
        !packer_present,
        if packer_present {
            "already installed".to_owned()
        } else {
            "from HashiCorp's apt repository, which this step adds".to_owned()
        },
    ));

    steps.push(
        Step::new(
            "kvm group",
            "sudo usermod -aG kvm \"$USER\"".to_owned(),
            !linux.in_kvm_group(),
            if linux.in_kvm_group() {
                "already a member".to_owned()
            } else {
                "write access to /dev/kvm comes from the kvm group".to_owned()
            },
        )
        .relogin(),
    );

    steps.push(ssh_key_step(inputs, store, HostOs::Linux));
    steps
}

fn ssh_key_step(inputs: &SetupInputs, store: &Store, host: HostOs) -> Step {
    let key = store.ssh_key();
    let script = if host == HostOs::Windows {
        format!(
            "New-Item -ItemType Directory -Force -Path {parent} | Out-Null\n\
             ssh-keygen -t ed25519 -N '\"\"' -C 'sunlit-e2e' -f {key}",
            parent = ps_quote(key.parent().unwrap_or(&key)),
            key = ps_quote(&key)
        )
    } else {
        format!(
            "mkdir -p {parent} && ssh-keygen -t ed25519 -N '' -C sunlit-e2e -f {key}",
            parent = shell_quote(&key.parent().unwrap_or(&key).to_string_lossy()),
            key = shell_quote(&key.to_string_lossy())
        )
    };
    Step::new(
        "guest ssh key",
        script,
        !inputs.ssh_key_present,
        if inputs.ssh_key_present {
            "already generated".to_owned()
        } else {
            "one key pair, baked into both golden images at build time".to_owned()
        },
    )
}

/// The script that provisions the WSL distribution, run as root inside it.
pub fn wsl_provision_script() -> String {
    // Runs as root, because apt needs it. The toolchain does not: the build
    // runs as the distribution's default user, and a rustup installed under
    // /root is unreadable to them, so it would be installed and then not
    // found. The default user is the uid 1000 account WSL creates; asking for
    // it by uid avoids depending on what it was named or on whether sudo
    // prompts for a password.
    format!(
        "set -e\n\
         export DEBIAN_FRONTEND=noninteractive\n\
         apt-get update\n\
         apt-get install -y curl {LINUX_BUILD_DEPS}\n\
         build_user=\"$(getent passwd 1000 | cut -d: -f1)\"\n\
         if [ -z \"$build_user\" ]; then\n  \
         echo 'no uid 1000 account in this distribution' >&2\n  \
         exit 1\n\
         fi\n\
         echo \"installing the toolchain for $build_user\"\n\
         su - \"$build_user\" -c 'command -v cargo >/dev/null 2>&1 || curl \
         --proto \"=https\" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'\n"
    )
}

/// Single-quote a value for `sh`.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Parse the `RESTART_NEEDED=True` line an optional-feature step prints.
pub fn parse_restart_needed(stdout: &str) -> Option<bool> {
    stdout.lines().find_map(|line| {
        let value = line.trim().strip_prefix("RESTART_NEEDED=")?;
        match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    })
}

/// What running the plan produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SetupOutcome {
    pub ran: Vec<&'static str>,
    pub skipped: Vec<&'static str>,
    pub failed: Vec<(&'static str, String)>,
    pub restart_needed: bool,
    pub relogin_needed: bool,
}

/// Run the steps that are needed, in order, stopping at nothing: a failure is
/// recorded and the rest still run, because most of them are independent and a
/// partial setup plus an accurate report beats an early exit.
pub fn execute(runner: &dyn Runner, steps: &[Step], host: HostOs) -> SetupOutcome {
    let mut outcome = SetupOutcome::default();
    for step in steps {
        if !step.needed {
            outcome.skipped.push(step.name);
            continue;
        }
        println!("== {} ({})", step.name, step.note);
        let cmd = if host == HostOs::Windows {
            powershell(&step.script)
        } else {
            Cmd::new("bash").args(["-lc", &step.script])
        };
        match runner.capture(&cmd) {
            Ok(output) if output.success() => {
                if !output.stdout.trim().is_empty() {
                    println!("{}", output.stdout.trim());
                }
                if step.may_need_restart && parse_restart_needed(&output.stdout).unwrap_or(true) {
                    outcome.restart_needed = true;
                }
                if step.needs_relogin {
                    outcome.relogin_needed = true;
                }
                outcome.ran.push(step.name);
            }
            Ok(output) => {
                let detail = if output.stderr.trim().is_empty() {
                    output.stdout.trim().to_owned()
                } else {
                    output.stderr.trim().to_owned()
                };
                outcome
                    .failed
                    .push((step.name, format!("exit {:?}: {detail}", output.code)));
            }
            Err(e) => outcome.failed.push((step.name, e.to_string())),
        }
    }
    outcome
}

/// The closing verdict, which is the whole point of the command reporting
/// rather than performing.
pub fn render_outcome(outcome: &SetupOutcome) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n{} run, {} already in place, {} failed",
        outcome.ran.len(),
        outcome.skipped.len(),
        outcome.failed.len()
    );
    for (name, error) in &outcome.failed {
        let _ = writeln!(out, "failed: {name}: {error}");
    }
    if outcome.restart_needed {
        let _ = writeln!(
            out,
            "\nA restart is needed before the hypervisor runs. This command does not perform it."
        );
    }
    if outcome.relogin_needed {
        let _ = writeln!(
            out,
            "\nSign out and back in before the new group membership and PATH reach your session. \
             This command does not perform it."
        );
    }
    let _ = writeln!(
        out,
        "\nThen run `cargo xtask vm doctor` to confirm, and \
         `cargo xtask vm build-image <target>` to build a guest."
    );
    out
}

/// The refusal an unelevated Windows shell gets.
pub fn elevation_message() -> String {
    "`cargo xtask vm setup` changes Windows features and group membership, so it needs an \
     elevated shell.\n\nStart one and run it again:\n  \
     Start-Process powershell -Verb RunAs\n  cd <repo>\n  cargo xtask vm setup\n\n\
     `cargo xtask vm doctor` works unelevated and changes nothing."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::facts::{LinuxFacts, WindowsFacts, WslDistro};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn store() -> Store {
        Store::new("/srv/vm")
    }

    fn fresh_windows() -> SetupInputs {
        SetupInputs {
            facts: HostFacts {
                os: Some(HostOs::Windows),
                windows: Some(WindowsFacts {
                    caption: "Microsoft Windows 11 Pro".to_owned(),
                    sku: 48,
                    ..WindowsFacts::default()
                }),
                tools: BTreeMap::new(),
                ..HostFacts::default()
            },
            wsl_ready: false,
            ssh_key_present: false,
        }
    }

    fn done_windows() -> SetupInputs {
        let mut inputs = fresh_windows();
        let windows = inputs.facts.windows.as_mut().unwrap();
        windows
            .features
            .insert(FEATURE_HYPERV.to_owned(), FeatureState::Enabled);
        windows
            .features
            .insert(FEATURE_WHPX.to_owned(), FeatureState::Enabled);
        windows.in_hyperv_admins = true;
        windows.machine_path = format!(r"C:\Windows\system32;{}", WINDOWS_QEMU_DIRS[0]);
        windows.wsl_distros = vec![WslDistro {
            name: WSL_DISTRO.to_owned(),
            state: "Stopped".to_owned(),
            version: 2,
        }];
        for tool in ["qemu-system-x86_64", "qemu-img", "packer"] {
            inputs
                .facts
                .tools
                .insert(tool.to_owned(), Some(PathBuf::from("C:/bin")));
        }
        inputs.facts.iso_tools = vec!["oscdimg".to_owned()];
        inputs.wsl_ready = true;
        inputs.ssh_key_present = true;
        inputs
    }

    #[test]
    fn a_fresh_windows_host_needs_every_step() {
        let steps = windows_plan(&fresh_windows(), &store());
        assert!(steps.iter().all(|s| s.needed), "{steps:#?}");
        let names: Vec<&str> = steps.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "hyper-v feature",
                "hypervisor platform",
                "qemu",
                "qemu on PATH",
                "iso builder",
                "packer",
                "hyper-v administrators",
                "wsl distro",
                "wsl build dependencies",
                "guest ssh key",
            ]
        );
    }

    #[test]
    fn running_setup_twice_is_a_no_op_the_second_time() {
        let steps = windows_plan(&done_windows(), &store());
        assert!(
            steps.iter().all(|s| !s.needed),
            "{:#?}",
            steps.iter().filter(|s| s.needed).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_second_run_in_the_same_shell_plans_nothing_for_the_winget_tools() {
        // The regression: winget appends its links directory to the user PATH,
        // so the shell that ran setup still cannot see `packer` or `oscdimg`.
        // Detection finds them anyway, through the link.
        let mut inputs = done_windows();
        inputs.facts.iso_tools.clear();
        inputs.facts.iso_tools_off_path = vec!["oscdimg".to_owned()];
        let steps = windows_plan(&inputs, &store());
        for name in ["iso builder", "packer", "qemu"] {
            let step = steps.iter().find(|s| s.name == name).unwrap();
            assert!(!step.needed, "{name} would be installed twice: {step:?}");
            assert_eq!(step.note, "already installed");
        }
    }

    #[test]
    fn a_winget_step_asks_before_it_installs_and_tolerates_a_package_that_is_there() {
        let script = winget_install_script(WINGET_PACKER);
        let list = script
            .find("winget list")
            .expect("it asks whether the package is installed");
        let install = script
            .find("winget install")
            .expect("and installs it if not");
        assert!(list < install, "the query comes first: {script}");
        // Both exits from the "already there" branches are successes, and the
        // real failure still exits non-zero.
        assert_eq!(script.matches("exit 0").count(), 2, "{script}");
        assert_eq!(script.matches("exit 1").count(), 1, "{script}");
        assert!(script.contains("$LASTEXITCODE"), "{script}");
        // A broken line continuation in one of these scripts is invisible in
        // review and reaches the user as a double space.
        assert!(!script.contains("  --"), "{script}");
    }

    #[test]
    fn a_finished_linux_host_needs_nothing_either() {
        let mut inputs = SetupInputs {
            facts: HostFacts {
                os: Some(HostOs::Linux),
                linux: Some(LinuxFacts {
                    groups: vec!["kvm".to_owned()],
                    ..LinuxFacts::default()
                }),
                ..HostFacts::default()
            },
            wsl_ready: true,
            ssh_key_present: true,
        };
        for tool in ["qemu-system-x86_64", "qemu-img", "packer"] {
            inputs
                .facts
                .tools
                .insert(tool.to_owned(), Some(PathBuf::from("/usr/bin")));
        }
        inputs.facts.iso_tools = vec!["xorriso".to_owned()];
        let steps = linux_plan(&inputs, &store());
        assert!(steps.iter().all(|s| !s.needed), "{steps:#?}");
    }

    #[test]
    fn the_qemu_path_step_is_separate_from_installing_qemu() {
        // An installed QEMU that is not on PATH is the state the winget package
        // leaves behind, and it has to be detected as such.
        let mut inputs = done_windows();
        inputs.facts.windows.as_mut().unwrap().machine_path = r"C:\Windows\system32".to_owned();
        let steps = windows_plan(&inputs, &store());
        let install = steps.iter().find(|s| s.name == "qemu").unwrap();
        let path = steps.iter().find(|s| s.name == "qemu on PATH").unwrap();
        assert!(!install.needed);
        assert!(path.needed);
        assert!(path.needs_relogin);
        assert!(path.script.contains("SetEnvironmentVariable"));
    }

    #[test]
    fn the_feature_steps_never_restart_and_report_whether_one_is_pending() {
        let steps = windows_plan(&fresh_windows(), &store());
        for name in ["hyper-v feature", "hypervisor platform"] {
            let step = steps.iter().find(|s| s.name == name).unwrap();
            assert!(step.script.contains("-NoRestart"), "{}", step.script);
            assert!(step.script.contains("RESTART_NEEDED"), "{}", step.script);
            assert!(step.may_need_restart);
            assert!(
                !step.script.contains("Restart-Computer"),
                "setup must never reboot"
            );
        }
    }

    #[test]
    fn no_step_on_either_host_reboots_or_logs_anyone_out() {
        let mut all = windows_plan(&fresh_windows(), &store());
        all.extend(linux_plan(&SetupInputs::default(), &store()));
        for step in &all {
            for forbidden in ["Restart-Computer", "shutdown", "logoff", "reboot"] {
                assert!(
                    !step.script.contains(forbidden),
                    "{} runs {forbidden}",
                    step.name
                );
            }
        }
    }

    #[test]
    fn the_winget_ids_are_the_verified_ones() {
        let steps = windows_plan(&fresh_windows(), &store());
        let qemu = steps.iter().find(|s| s.name == "qemu").unwrap();
        let packer = steps.iter().find(|s| s.name == "packer").unwrap();
        assert!(qemu.script.contains("SoftwareFreedomConservancy.QEMU"));
        assert!(packer.script.contains("Hashicorp.Packer"));
        for step in [qemu, packer] {
            assert!(step.script.contains("--accept-source-agreements"));
            assert!(step.script.contains("--silent"));
        }
    }

    #[test]
    fn the_wsl_step_uses_the_launcher_because_no_launch_leaves_it_unregistered() {
        let steps = windows_plan(&fresh_windows(), &store());
        let step = steps.iter().find(|s| s.name == "wsl distro").unwrap();
        assert!(step.script.contains("--no-launch"), "{}", step.script);
        assert!(
            step.script.contains("ubuntu2204.exe install --root"),
            "{}",
            step.script
        );
    }

    #[test]
    fn the_wsl_provisioning_script_installs_the_documented_dependencies() {
        let script = wsl_provision_script();
        for package in ["clang", "libclang-dev", "mesa-vulkan-drivers", "xvfb"] {
            assert!(script.contains(package), "missing {package}");
        }
        assert!(
            script.contains("rustup"),
            "the guest binaries need a toolchain"
        );
        assert!(script.contains("DEBIAN_FRONTEND=noninteractive"));
    }

    #[test]
    fn provisioning_installs_the_toolchain_for_the_account_that_builds() {
        // Three places have to agree on which WSL account matters: this one
        // installs the toolchain, `wsl_ready` probes for it, and
        // `artifacts::wsl_build_command` uses it. Only apt needs root.
        let script = wsl_provision_script();
        assert!(script.contains("getent passwd 1000"), "{script}");
        assert!(script.contains("su - \"$build_user\""), "{script}");
        assert!(script.contains("rustup.rs"), "{script}");
        // The build command names no user, so it runs as the default one.
        let build = crate::guest::artifacts::wsl_build_command(WSL_DISTRO, "/mnt/c/x");
        assert!(
            !build.args.iter().any(|a| a == "--user"),
            "{:?}",
            build.args
        );
    }

    #[test]
    fn the_restart_flag_is_read_from_the_step_output() {
        assert_eq!(parse_restart_needed("RESTART_NEEDED=True"), Some(true));
        assert_eq!(
            parse_restart_needed("noise\nRESTART_NEEDED=False\n"),
            Some(false)
        );
        assert_eq!(parse_restart_needed("nothing here"), None);
    }

    #[test]
    fn a_step_whose_output_is_unreadable_assumes_a_restart_is_pending() {
        use crate::runner::CommandOutput;
        use crate::runner::fake::FakeRunner;
        let steps = windows_plan(&fresh_windows(), &store());
        let runner = FakeRunner::new()
            .on("Enable-WindowsOptionalFeature", CommandOutput::ok(""))
            .on("winget", CommandOutput::ok(""))
            .on("SetEnvironmentVariable", CommandOutput::ok(""))
            .on("Get-LocalGroup", CommandOutput::ok(""))
            .on("wsl.exe --install", CommandOutput::ok(""))
            .on("wsl.exe -d", CommandOutput::ok(""))
            .on("ssh-keygen", CommandOutput::ok(""));
        let outcome = execute(&runner, &steps, HostOs::Windows);
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        assert_eq!(outcome.ran.len(), steps.len());
        assert!(outcome.restart_needed);
        assert!(outcome.relogin_needed);
    }

    #[test]
    fn a_failing_step_does_not_stop_the_others() {
        use crate::runner::CommandOutput;
        use crate::runner::fake::FakeRunner;
        let steps = windows_plan(&fresh_windows(), &store());
        let runner = FakeRunner::new()
            .on(
                "Enable-WindowsOptionalFeature",
                CommandOutput::failed(1, "access denied"),
            )
            .on("winget", CommandOutput::ok(""))
            .on("SetEnvironmentVariable", CommandOutput::ok(""))
            .on("Get-LocalGroup", CommandOutput::ok(""))
            .on("wsl.exe --install", CommandOutput::ok(""))
            .on("wsl.exe -d", CommandOutput::ok(""))
            .on("ssh-keygen", CommandOutput::ok(""));
        let outcome = execute(&runner, &steps, HostOs::Windows);
        assert_eq!(outcome.failed.len(), 2, "{outcome:?}");
        assert!(outcome.failed[0].1.contains("access denied"));
        assert_eq!(outcome.ran.len(), steps.len() - 2);
        let text = render_outcome(&outcome);
        assert!(text.contains("failed: hyper-v feature"), "{text}");
        assert!(text.contains("vm doctor"), "{text}");
    }

    #[test]
    fn skipped_steps_are_reported_as_already_in_place() {
        use crate::runner::fake::FakeRunner;
        let steps = windows_plan(&done_windows(), &store());
        let runner = FakeRunner::new();
        let outcome = execute(&runner, &steps, HostOs::Windows);
        assert!(outcome.ran.is_empty());
        assert_eq!(outcome.skipped.len(), steps.len());
        assert!(!outcome.restart_needed);
        assert!(!outcome.relogin_needed);
        assert!(render_outcome(&outcome).contains("0 run,"));
    }

    #[test]
    fn the_elevation_refusal_says_how_to_get_an_elevated_shell() {
        let message = elevation_message();
        assert!(message.contains("RunAs"));
        assert!(message.contains("vm doctor"));
    }
}
