//! `cargo xtask vm doctor`: the single verification path.
//!
//! Unelevated, read-only, and the one place that answers "can this host run the
//! VM suite". It prints a line per check and exits nonzero if any of them
//! failed. Warnings are for things that block one target or one convenience,
//! failures for things that block everything.
//!
//! [`evaluate`] is a pure function from facts to a report, so every branch in
//! here is tested against a fabricated host rather than against the machine the
//! tests happen to run on.

use std::fmt::Write as _;

use crate::facts::{
    FEATURE_HYPERV, FEATURE_WHPX, FeatureState, HostFacts, REQUIRED_TOOLS, WSL_DISTRO,
};
use crate::inventory::{ImageCondition, Inventory};
use crate::runner::Runner;
use crate::target::{HostOs, Target};
use crate::util::{self, format_bytes};

/// Below this, nothing will fit and the doctor says so.
pub const MIN_FREE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Below this, one of the two images fits but not both plus their overlays.
pub const RECOMMENDED_FREE_BYTES: u64 = 80 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    /// What to do about it, printed under the line.
    pub hint: Option<String>,
}

impl Check {
    fn new(name: impl Into<String>, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
            hint: None,
        }
    }

    #[must_use]
    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn worst(&self) -> Status {
        self.checks
            .iter()
            .map(|c| c.status)
            .max()
            .unwrap_or(Status::Pass)
    }

    pub fn count(&self, status: Status) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }

    pub fn failed(&self) -> bool {
        self.worst() == Status::Fail
    }

    /// Find a check by name, which is how the tests assert on one line without
    /// depending on the order of the rest.
    pub fn get(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.name == name)
    }

    pub fn render(&self) -> String {
        let width = self
            .checks
            .iter()
            .map(|c| c.name.len())
            .max()
            .unwrap_or(0)
            .min(34);
        let mut out = String::new();
        for check in &self.checks {
            let _ = writeln!(
                out,
                "{}  {:width$}  {}",
                check.status.label(),
                check.name,
                check.detail
            );
            if let Some(hint) = &check.hint {
                let _ = writeln!(out, "      {:width$}  -> {hint}", "");
            }
        }
        let _ = writeln!(
            out,
            "\n{} passed, {} warnings, {} failed",
            self.count(Status::Pass),
            self.count(Status::Warn),
            self.count(Status::Fail)
        );
        let _ = writeln!(
            out,
            "{}",
            match self.worst() {
                Status::Fail =>
                    "This host cannot run the VM suite yet. Fix the failures above, \
                                 starting with `cargo xtask vm setup` in an elevated shell.",
                Status::Warn => "This host can run what the warnings above do not exclude.",
                Status::Pass => "This host is ready.",
            }
        );
        out
    }
}

/// Turn facts into a verdict.
pub fn evaluate(facts: &HostFacts, inventory: &Inventory, now_unix: u64) -> Report {
    let mut checks = Vec::new();

    if let Some(error) = &facts.probe_error {
        checks.push(
            Check::new("host probe", Status::Fail, error.clone())
                .hint("without it the host checks below cannot be answered"),
        );
    }

    match facts.os() {
        HostOs::Windows => {
            checks.push(Check::new(
                "host",
                Status::Pass,
                format!("{} (Windows host)", facts.description),
            ));
            windows_checks(facts, &mut checks);
        }
        HostOs::Linux => {
            checks.push(Check::new(
                "host",
                Status::Pass,
                format!("{} (Linux host)", facts.description),
            ));
            linux_checks(facts, &mut checks);
        }
        HostOs::Other => {
            checks.push(
                Check::new(
                    "host",
                    Status::Fail,
                    format!("{} is not a supported VM host", facts.description),
                )
                .hint(
                    "macOS coverage runs on hosted CI runners; \
                     `cargo xtask e2e --target host` still works here",
                ),
            );
        }
    }

    tool_checks(facts, &mut checks);
    disk_check(facts, &mut checks);
    image_checks(inventory, now_unix, &mut checks);

    Report { checks }
}

fn windows_checks(facts: &HostFacts, checks: &mut Vec<Check>) {
    let Some(windows) = &facts.windows else {
        return;
    };
    windows_hypervisor_checks(windows, checks);
    windows_access_checks(windows, checks);
}

/// Can this host run a hypervisor at all, and is one running?
fn windows_hypervisor_checks(windows: &crate::facts::WindowsFacts, checks: &mut Vec<Check>) {
    checks.push(if windows.edition_supports_hyperv() {
        Check::new("windows edition", Status::Pass, "supports Hyper-V")
    } else {
        Check::new(
            "windows edition",
            Status::Fail,
            format!("{} has no Hyper-V", windows.caption),
        )
        .hint("the Windows guest needs Pro, Enterprise, or Education")
    });

    let hyperv = windows.feature(FEATURE_HYPERV);
    checks.push(match hyperv {
        FeatureState::Enabled => Check::new("hyper-v feature", Status::Pass, "enabled"),
        state => Check::new("hyper-v feature", Status::Fail, state.label())
            .hint("`cargo xtask vm setup` enables it; it needs a restart afterwards"),
    });

    let whpx = windows.feature(FEATURE_WHPX);
    checks.push(match whpx {
        FeatureState::Enabled => Check::new("hypervisor platform", Status::Pass, "enabled"),
        state => Check::new("hypervisor platform", Status::Fail, state.label()).hint(
            "QEMU's WHPX acceleration needs it; `cargo xtask vm setup` enables it \
             alongside Hyper-V",
        ),
    });

    checks.push(if windows.hypervisor_present {
        Check::new(
            "hypervisor running",
            Status::Pass,
            "a hypervisor is running",
        )
    } else if hyperv == FeatureState::Enabled {
        Check::new(
            "hypervisor running",
            Status::Warn,
            "the features are enabled but no hypervisor is running",
        )
        .hint("this is what a pending restart looks like; restart, then run the doctor again")
    } else if windows.virtualization_firmware_enabled == Some(false) {
        Check::new(
            "hypervisor running",
            Status::Fail,
            "no hypervisor, and the firmware reports virtualization disabled",
        )
        .hint("enable VT-x or AMD-V in the firmware setup")
    } else {
        Check::new(
            "hypervisor running",
            Status::Warn,
            "no hypervisor is running yet",
        )
        .hint("expected until the Hyper-V feature is enabled and the host restarted")
    });
}

/// Is the management service there, can this session drive it, and is the WSL
/// distribution that builds the Linux guest's binaries in place?
fn windows_access_checks(windows: &crate::facts::WindowsFacts, checks: &mut Vec<Check>) {
    checks.push(
        match (
            windows.feature(FEATURE_HYPERV),
            windows.vmms_state.as_deref(),
        ) {
            (FeatureState::Enabled, Some("Running")) => {
                Check::new("vmms service", Status::Pass, "running")
            }
            (FeatureState::Enabled, Some(state)) => {
                Check::new("vmms service", Status::Warn, format!("state is {state}")).hint(
                    "the Hyper-V management service is installed but not running; it is \
                     trigger-started, so this only matters if starting a VM then fails",
                )
            }
            (FeatureState::Enabled, None) => Check::new(
                "vmms service",
                Status::Fail,
                "the Hyper-V management service is not installed",
            ),
            (_, _) => Check::new(
                "vmms service",
                Status::Warn,
                "not present, which follows from the Hyper-V feature above",
            ),
        },
    );

    checks.push(if windows.in_hyperv_admins {
        Check::new(
            "hyper-v administrators",
            Status::Pass,
            "the current session is a member",
        )
    } else {
        Check::new(
            "hyper-v administrators",
            Status::Fail,
            "the current session is not a member",
        )
        .hint(
            "`cargo xtask vm setup` adds you; the membership only reaches your token \
             after signing out and back in",
        )
    });

    checks.push(match windows.distro(WSL_DISTRO) {
        Some(distro) if distro.version >= 2 => Check::new(
            "wsl distro",
            Status::Pass,
            format!("{} is registered (WSL {})", distro.name, distro.version),
        ),
        Some(distro) => Check::new(
            "wsl distro",
            Status::Fail,
            format!("{} is on WSL {}", distro.name, distro.version),
        )
        .hint(format!("`wsl --set-version {WSL_DISTRO} 2`")),
        None => Check::new(
            "wsl distro",
            Status::Warn,
            format!("{WSL_DISTRO} is not registered"),
        )
        .hint("only `cargo xtask e2e --target linux` needs it; `cargo xtask vm setup` installs it"),
    });
}

fn linux_checks(facts: &HostFacts, checks: &mut Vec<Check>) {
    let Some(linux) = &facts.linux else {
        return;
    };

    checks.push(if !linux.kvm_present {
        Check::new("kvm", Status::Fail, "/dev/kvm does not exist")
            .hint("enable virtualization in the firmware and load the kvm_intel or kvm_amd module")
    } else if linux.kvm_writable {
        Check::new("kvm", Status::Pass, "/dev/kvm is writable")
    } else if linux.in_kvm_group() {
        Check::new(
            "kvm",
            Status::Fail,
            "/dev/kvm exists and you are in the kvm group, but it is not writable",
        )
        .hint("check the device permissions: udev usually grants the kvm group write access")
    } else {
        Check::new("kvm", Status::Fail, "/dev/kvm is not writable")
            .hint("`cargo xtask vm setup` adds you to the kvm group; log out and back in after")
    });
}

fn tool_checks(facts: &HostFacts, checks: &mut Vec<Check>) {
    for tool in REQUIRED_TOOLS {
        checks.push(match facts.tool(tool) {
            Some(path) => Check::new(tool, Status::Pass, path.display().to_string()),
            None => Check::new(tool, Status::Fail, "not found").hint(hint_for_tool(tool)),
        });
    }

    checks.push(match &facts.vnc_viewer {
        Some(path) => Check::new("vnc viewer", Status::Pass, path.display().to_string()),
        None => Check::new("vnc viewer", Status::Warn, "none found").hint(
            "only `cargo xtask vm view` of a QEMU guest needs one; \
             without it the VNC address is printed instead",
        ),
    });

    if facts.os() == HostOs::Windows {
        checks.push(match facts.tool("vmconnect") {
            Some(path) => Check::new("vmconnect", Status::Pass, path.display().to_string()),
            None => Check::new("vmconnect", Status::Warn, "not found").hint(
                "`cargo xtask vm view windows` uses it; it ships with the Hyper-V \
                 management tools",
            ),
        });
    }
}

fn hint_for_tool(tool: &str) -> String {
    match tool {
        "packer" => {
            "`cargo xtask vm setup` installs it (winget package Hashicorp.Packer)".to_owned()
        }
        _ if tool.starts_with("qemu") => format!(
            "`cargo xtask vm setup` installs QEMU (winget package \
             SoftwareFreedomConservancy.QEMU) and puts {tool} on PATH; \
             the installer itself does neither"
        ),
        _ => format!("{tool} ships with the OpenSSH client; install it and put it on PATH"),
    }
}

fn disk_check(facts: &HostFacts, checks: &mut Vec<Check>) {
    checks.push(match facts.free_bytes {
        None => Check::new("disk space", Status::Warn, "could not be determined"),
        Some(free) if free < MIN_FREE_BYTES => Check::new(
            "disk space",
            Status::Fail,
            format!(
                "{} free, less than the {} minimum",
                format_bytes(free),
                format_bytes(MIN_FREE_BYTES)
            ),
        )
        .hint("`cargo xtask vm status` shows what the image store is using"),
        Some(free) if free < RECOMMENDED_FREE_BYTES => Check::new(
            "disk space",
            Status::Warn,
            format!(
                "{} free; both images plus overlays want about {}",
                format_bytes(free),
                format_bytes(RECOMMENDED_FREE_BYTES)
            ),
        ),
        Some(free) => Check::new(
            "disk space",
            Status::Pass,
            format!("{} free", format_bytes(free)),
        ),
    });
}

fn image_checks(inventory: &Inventory, now_unix: u64, checks: &mut Vec<Check>) {
    for target in Target::ALL {
        let name = format!("{target} image");
        let Some(entry) = inventory.for_target(target) else {
            checks.push(Check::new(name, Status::Warn, "not inspected"));
            continue;
        };
        let condition = entry.condition(now_unix);
        let detail = format!("{}: {}", condition.label(), condition.detail());
        let check = match condition {
            ImageCondition::Ok => Check::new(name, Status::Pass, detail),
            ImageCondition::Corrupt { .. } | ImageCondition::Expired { .. } => {
                Check::new(name, Status::Fail, detail)
                    .hint(format!("`cargo xtask vm build-image {target}`"))
            }
            ImageCondition::Missing
            | ImageCondition::Stale { .. }
            | ImageCondition::Expiring { .. }
            | ImageCondition::Unmanifested { .. } => Check::new(name, Status::Warn, detail)
                .hint(format!("`cargo xtask vm build-image {target}`")),
        };
        checks.push(check);
    }
}

/// Collect, evaluate, print. Returns the process exit code.
pub fn run(runner: &dyn Runner) -> Result<u8, String> {
    let store = crate::store::store()?;
    let facts = crate::facts::collect(runner, HostOs::current(), store.root());
    let inventory = crate::inventory::scan(&store);
    let report = evaluate(&facts, &inventory, util::now_unix());

    println!("host checks for the sunlit-earth VM suite");
    println!("image store: {}", store.root().display());
    println!();
    print!("{}", report.render());

    Ok(u8::from(report.failed()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{LinuxFacts, WindowsFacts, WslDistro};
    use crate::inventory::fixtures::{BUILT, empty, healthy, inventory};
    use crate::util::SECS_PER_DAY;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn features(hyperv: i32, whpx: i32) -> BTreeMap<String, FeatureState> {
        let mut map = BTreeMap::new();
        map.insert(FEATURE_HYPERV.to_owned(), FeatureState::from_code(hyperv));
        map.insert(FEATURE_WHPX.to_owned(), FeatureState::from_code(whpx));
        map
    }

    /// A Windows host with everything in place.
    fn good_windows() -> HostFacts {
        let mut tools = BTreeMap::new();
        for tool in REQUIRED_TOOLS {
            tools.insert(
                tool.to_owned(),
                Some(PathBuf::from(format!("C:/bin/{tool}.exe"))),
            );
        }
        tools.insert(
            "vmconnect".to_owned(),
            Some(PathBuf::from("C:/Windows/System32/vmconnect.exe")),
        );
        HostFacts {
            os: Some(HostOs::Windows),
            description: "Microsoft Windows 11 Pro 10.0.26200".to_owned(),
            windows: Some(WindowsFacts {
                caption: "Microsoft Windows 11 Pro".to_owned(),
                version: "10.0.26200".to_owned(),
                sku: 48,
                hypervisor_present: true,
                virtualization_firmware_enabled: None,
                features: features(1, 1),
                vmms_state: Some("Running".to_owned()),
                in_hyperv_admins: true,
                elevated: false,
                wsl_distros: vec![WslDistro {
                    name: WSL_DISTRO.to_owned(),
                    state: "Stopped".to_owned(),
                    version: 2,
                }],
                machine_path: r"C:\Windows\system32;C:\Program Files\qemu".to_owned(),
            }),
            tools,
            vnc_viewer: Some(PathBuf::from("C:/bin/vncviewer.exe")),
            free_bytes: Some(500 * 1024 * 1024 * 1024),
            ..HostFacts::default()
        }
    }

    fn good_linux() -> HostFacts {
        let mut tools = BTreeMap::new();
        for tool in REQUIRED_TOOLS {
            tools.insert(
                tool.to_owned(),
                Some(PathBuf::from(format!("/usr/bin/{tool}"))),
            );
        }
        HostFacts {
            os: Some(HostOs::Linux),
            description: "Ubuntu 22.04.4 LTS".to_owned(),
            linux: Some(LinuxFacts {
                pretty_name: "Ubuntu 22.04.4 LTS".to_owned(),
                kvm_present: true,
                kvm_writable: true,
                groups: vec!["kvm".to_owned()],
            }),
            tools,
            vnc_viewer: Some(PathBuf::from("/usr/bin/vncviewer")),
            free_bytes: Some(500 * 1024 * 1024 * 1024),
            ..HostFacts::default()
        }
    }

    fn built_images() -> Inventory {
        inventory(vec![healthy(Target::Windows), healthy(Target::Linux)])
    }

    fn now() -> u64 {
        BUILT + SECS_PER_DAY
    }

    #[test]
    fn a_ready_windows_host_with_images_passes_everything() {
        let report = evaluate(&good_windows(), &built_images(), now());
        assert_eq!(report.worst(), Status::Pass, "{}", report.render());
        assert!(!report.failed());
        assert!(report.render().contains("This host is ready."));
    }

    #[test]
    fn a_ready_linux_host_passes_everything() {
        let report = evaluate(&good_linux(), &built_images(), now());
        assert_eq!(report.worst(), Status::Pass, "{}", report.render());
    }

    #[test]
    fn the_linux_host_is_not_asked_about_hyper_v_or_wsl() {
        let report = evaluate(&good_linux(), &built_images(), now());
        assert!(report.get("hyper-v feature").is_none());
        assert!(report.get("wsl distro").is_none());
        assert!(report.get("kvm").is_some());
    }

    #[test]
    fn a_disabled_hypervisor_platform_feature_fails_and_names_the_fix() {
        let mut facts = good_windows();
        facts.windows.as_mut().unwrap().features = features(1, 2);
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("hypervisor platform").expect("checked");
        assert_eq!(check.status, Status::Fail);
        assert!(check.hint.as_ref().unwrap().contains("vm setup"));
        assert!(report.failed());
    }

    #[test]
    fn features_enabled_without_a_running_hypervisor_reads_as_a_pending_restart() {
        let mut facts = good_windows();
        let windows = facts.windows.as_mut().unwrap();
        windows.hypervisor_present = false;
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("hypervisor running").expect("checked");
        assert_eq!(check.status, Status::Warn);
        assert!(check.hint.as_ref().unwrap().contains("restart"));
    }

    #[test]
    fn firmware_virtualization_is_only_consulted_when_no_hypervisor_runs() {
        // With a hypervisor present, a false firmware flag is meaningless:
        // Hyper-V owns VT-x and the flag reads false on plenty of hosts.
        let mut facts = good_windows();
        facts
            .windows
            .as_mut()
            .unwrap()
            .virtualization_firmware_enabled = Some(false);
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(
            report.get("hypervisor running").unwrap().status,
            Status::Pass
        );

        // With nothing running and nothing enabled, it is the diagnosis.
        let windows = facts.windows.as_mut().unwrap();
        windows.hypervisor_present = false;
        windows.features = features(2, 2);
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("hypervisor running").unwrap();
        assert_eq!(check.status, Status::Fail);
        assert!(check.detail.contains("firmware"));
    }

    #[test]
    fn the_management_service_warns_when_idle_and_fails_when_absent() {
        let mut facts = good_windows();
        let windows = facts.windows.as_mut().unwrap();
        windows.vmms_state = Some("Stopped".to_owned());
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("vmms service").unwrap().status, Status::Warn);

        facts.windows.as_mut().unwrap().vmms_state = None;
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("vmms service").unwrap().status, Status::Fail);

        // With the feature off, its absence follows and is not reported twice.
        let windows = facts.windows.as_mut().unwrap();
        windows.features = features(2, 2);
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("vmms service").unwrap().status, Status::Warn);
    }

    #[test]
    fn a_home_edition_fails_the_edition_check() {
        let mut facts = good_windows();
        let windows = facts.windows.as_mut().unwrap();
        windows.caption = "Microsoft Windows 11 Home".to_owned();
        windows.sku = 101;
        windows.features = features(3, 3);
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("windows edition").unwrap().status, Status::Fail);
        assert!(
            report
                .get("hyper-v feature")
                .unwrap()
                .detail
                .contains("edition")
        );
    }

    #[test]
    fn missing_group_membership_fails_and_explains_the_relogin() {
        let mut facts = good_windows();
        facts.windows.as_mut().unwrap().in_hyperv_admins = false;
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("hyper-v administrators").expect("checked");
        assert_eq!(check.status, Status::Fail);
        assert!(check.hint.as_ref().unwrap().contains("signing out"));
    }

    #[test]
    fn a_missing_wsl_distro_warns_because_it_only_blocks_the_linux_target() {
        let mut facts = good_windows();
        facts.windows.as_mut().unwrap().wsl_distros.clear();
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("wsl distro").expect("checked");
        assert_eq!(check.status, Status::Warn);
        assert!(!report.failed());
    }

    #[test]
    fn a_wsl1_distro_fails_because_the_binaries_would_not_match_the_guest() {
        let mut facts = good_windows();
        facts.windows.as_mut().unwrap().wsl_distros = vec![WslDistro {
            name: WSL_DISTRO.to_owned(),
            state: "Stopped".to_owned(),
            version: 1,
        }];
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("wsl distro").unwrap().status, Status::Fail);
    }

    #[test]
    fn a_missing_tool_fails_and_the_qemu_hint_mentions_the_path_problem() {
        let mut facts = good_windows();
        facts.tools.insert("qemu-img".to_owned(), None);
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("qemu-img").expect("checked");
        assert_eq!(check.status, Status::Fail);
        assert!(check.hint.as_ref().unwrap().contains("PATH"));
    }

    #[test]
    fn a_missing_vnc_viewer_is_only_a_warning() {
        let mut facts = good_linux();
        facts.vnc_viewer = None;
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("vnc viewer").unwrap().status, Status::Warn);
        assert!(!report.failed());
    }

    #[test]
    fn disk_space_has_three_bands() {
        let mut facts = good_linux();
        facts.free_bytes = Some(10 * 1024 * 1024 * 1024);
        assert_eq!(
            evaluate(&facts, &built_images(), now())
                .get("disk space")
                .unwrap()
                .status,
            Status::Fail
        );

        facts.free_bytes = Some(40 * 1024 * 1024 * 1024);
        assert_eq!(
            evaluate(&facts, &built_images(), now())
                .get("disk space")
                .unwrap()
                .status,
            Status::Warn
        );

        facts.free_bytes = None;
        assert_eq!(
            evaluate(&facts, &built_images(), now())
                .get("disk space")
                .unwrap()
                .status,
            Status::Warn
        );
    }

    #[test]
    fn a_host_with_no_images_warns_rather_than_failing() {
        let empty_store = inventory(vec![empty(Target::Windows), empty(Target::Linux)]);
        let report = evaluate(&good_windows(), &empty_store, now());
        assert_eq!(report.get("windows image").unwrap().status, Status::Warn);
        assert_eq!(report.get("linux image").unwrap().status, Status::Warn);
        assert!(!report.failed(), "{}", report.render());
        assert!(
            report
                .get("linux image")
                .unwrap()
                .hint
                .as_ref()
                .unwrap()
                .contains("build-image linux")
        );
    }

    #[test]
    fn a_stale_image_warns_and_an_expired_one_fails() {
        let mut stale = healthy(Target::Linux);
        stale.template_hash = Some("crc32:ffffffff".to_owned());
        let report = evaluate(
            &good_windows(),
            &inventory(vec![healthy(Target::Windows), stale]),
            now(),
        );
        assert_eq!(report.get("linux image").unwrap().status, Status::Warn);

        let expired = evaluate(&good_windows(), &built_images(), BUILT + 100 * SECS_PER_DAY);
        let check = expired.get("windows image").expect("checked");
        assert_eq!(check.status, Status::Fail);
        assert!(check.detail.contains("expired"));
        assert!(expired.failed());
    }

    #[test]
    fn a_corrupt_image_fails() {
        let mut corrupt = healthy(Target::Windows);
        corrupt.images[0].bytes = 1;
        let report = evaluate(
            &good_windows(),
            &inventory(vec![corrupt, healthy(Target::Linux)]),
            now(),
        );
        assert_eq!(report.get("windows image").unwrap().status, Status::Fail);
    }

    #[test]
    fn a_failed_host_probe_is_reported_rather_than_read_as_a_clean_host() {
        let facts = HostFacts {
            os: Some(HostOs::Windows),
            probe_error: Some("cannot run powershell.exe".to_owned()),
            ..HostFacts::default()
        };
        let report = evaluate(&facts, &built_images(), now());
        assert_eq!(report.get("host probe").unwrap().status, Status::Fail);
        // Nothing was invented about the host it could not read.
        assert!(report.get("hyper-v feature").is_none());
    }

    #[test]
    fn an_unsupported_host_fails_and_points_at_the_host_target() {
        let facts = HostFacts {
            os: Some(HostOs::Other),
            description: "macos".to_owned(),
            ..HostFacts::default()
        };
        let report = evaluate(&facts, &built_images(), now());
        let check = report.get("host").expect("checked");
        assert_eq!(check.status, Status::Fail);
        assert!(check.hint.as_ref().unwrap().contains("--target host"));
    }

    #[test]
    fn the_rendered_report_lines_up_and_carries_the_hints() {
        let mut facts = good_windows();
        facts.tools.insert("packer".to_owned(), None);
        let text = evaluate(&facts, &built_images(), now()).render();
        assert!(text.contains("FAIL  packer"), "{text}");
        assert!(
            text.contains("-> `cargo xtask vm setup` installs it"),
            "{text}"
        );
        assert!(text.contains("1 failed"), "{text}");
        assert!(text.contains("elevated shell"), "{text}");
    }
}
