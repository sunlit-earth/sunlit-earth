//! `cargo xtask vm status`: what exists, what is running, and what it costs.
//!
//! Unelevated and read-only like the doctor. Plan decision 13 asks for one
//! place that answers "what is on my disk, what is running, and how do I get
//! rid of it", with the command to do so printed next to every number.

use std::fmt::Write as _;

use crate::inventory::{Inventory, TargetInventory};
use crate::state::RunState;
use crate::target::Target;
use crate::util::{format_bytes, format_unix_utc};

/// Render the whole inventory.
pub fn render(inventory: &Inventory, now_unix: u64) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "image store: {}", inventory.root.display());
    if !inventory.root_exists {
        let _ = writeln!(
            out,
            "  (does not exist yet; `cargo xtask vm build-image <target>` creates it)"
        );
    }
    let _ = writeln!(out);

    for target in Target::ALL {
        let entry = inventory.for_target(target);
        let _ = writeln!(out, "{}", target_section(target, entry, now_unix));
    }

    if inventory.iso.is_empty() {
        let _ = writeln!(out, "installation media: none cached");
    } else {
        let _ = writeln!(
            out,
            "installation media: {} ({})",
            crate::util::count(inventory.iso.len(), "file"),
            format_bytes(inventory.iso_bytes())
        );
        for file in &inventory.iso {
            let _ = writeln!(out, "  {}  {}", file.name(), format_bytes(file.bytes));
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "total: {}", format_bytes(inventory.total_bytes()));
    let images: u64 = inventory
        .targets
        .iter()
        .map(|t| t.image_bytes() + t.build_bytes())
        .sum();
    let runs: u64 = inventory
        .targets
        .iter()
        .map(TargetInventory::run_bytes)
        .sum();
    let _ = writeln!(
        out,
        "  {} in golden images and media, {} in run state",
        format_bytes(images + inventory.iso_bytes()),
        format_bytes(runs)
    );
    if runs > 0 {
        let _ = writeln!(
            out,
            "  `cargo xtask vm destroy all` frees the run state and leaves the images alone"
        );
    }
    if images + inventory.iso_bytes() > 0 {
        let _ = writeln!(
            out,
            "  `cargo xtask vm destroy all --purge` frees everything, including the images; \
             rebuilding costs one `vm build-image` per target"
        );
    }
    out
}

fn target_section(target: Target, entry: Option<&TargetInventory>, now_unix: u64) -> String {
    let mut out = String::new();
    let Some(entry) = entry else {
        let _ = writeln!(out, "{target}: not inspected");
        return out;
    };

    let condition = entry.condition(now_unix);
    let _ = writeln!(
        out,
        "{target}: image {} ({})",
        condition.label(),
        condition.detail()
    );

    for file in &entry.images {
        let _ = writeln!(
            out,
            "  {}  {}  {}",
            file.name(),
            format_bytes(file.bytes),
            file.path.display()
        );
    }
    if let Some(manifest) = &entry.manifest {
        let _ = writeln!(
            out,
            "  built {} from {} with {}",
            if manifest.built_utc.is_empty() {
                format_unix_utc(manifest.built_unix)
            } else {
                manifest.built_utc.clone()
            },
            if manifest.source.is_empty() {
                "an unrecorded source"
            } else {
                &manifest.source
            },
            if manifest.builder.is_empty() {
                "an unrecorded builder"
            } else {
                &manifest.builder
            },
        );
    }
    if let Some(error) = &entry.manifest_error {
        let _ = writeln!(out, "  manifest problem: {error}");
    }

    let _ = write!(out, "{}", run_state_section(target, entry));

    let _ = writeln!(
        out,
        "  footprint: {} image, {} run state",
        format_bytes(entry.image_bytes()),
        format_bytes(entry.run_bytes())
    );
    out
}

/// The half of a target's section that describes overlays and VMs.
fn run_state_section(target: Target, entry: &TargetInventory) -> String {
    let mut out = String::new();
    if entry.build_bytes() > 0 {
        let _ = writeln!(
            out,
            "  build leftovers: {} ({})",
            crate::util::count(entry.build_files.len(), "file"),
            format_bytes(entry.build_bytes())
        );
    }

    if entry.run_files.is_empty() {
        let _ = writeln!(out, "  no overlays or run state");
    } else {
        let _ = writeln!(
            out,
            "  run state: {} ({})",
            crate::util::count(entry.run_files.len(), "file"),
            format_bytes(entry.run_bytes())
        );
        for file in &entry.run_files {
            let _ = writeln!(out, "    {}  {}", file.name(), format_bytes(file.bytes));
        }
    }

    match (&entry.state, entry.running) {
        (Some(state), Some(true)) => {
            let _ = write!(out, "{}", running_vm(target, state));
        }
        (Some(state), Some(false)) => {
            let _ = writeln!(
                out,
                "  {} is registered but not running, left behind by a {}",
                state.vm_name,
                state.reason.label()
            );
            let _ = writeln!(
                out,
                "    `cargo xtask vm destroy {target}` removes the leftovers"
            );
        }
        (Some(state), None) => {
            let _ = writeln!(
                out,
                "  {} is recorded ({}); liveness not checked",
                state.vm_name,
                state.reason.label()
            );
            let _ = writeln!(out, "    `cargo xtask vm destroy {target}` removes it");
        }
        (None, _) => {}
    }

    if let Some(error) = &entry.state_error {
        let _ = writeln!(out, "  state file problem: {error}");
    }
    out
}

fn running_vm(target: Target, state: &RunState) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "  {} is running, started {} ({})",
        state.vm_name,
        format_unix_utc(state.started_unix),
        state.reason.label()
    );
    if state.ssh_port > 0 {
        let _ = writeln!(
            out,
            "    ssh:     `cargo xtask vm ssh {target}`  ({}:{})",
            state.ssh_host, state.ssh_port
        );
    }
    let _ = writeln!(
        out,
        "    desktop: `cargo xtask vm view {target}`{}",
        state
            .vnc
            .as_ref()
            .map(|vnc| format!("  (vnc {vnc})"))
            .unwrap_or_default()
    );
    let _ = writeln!(
        out,
        "    destroy: `cargo xtask vm destroy {target}`  (frees the memory and the overlay; \
         the golden image is untouched)"
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::FileInfo;
    use crate::inventory::fixtures::{BUILT, empty, healthy, inventory};
    use crate::state::StartReason;
    use crate::target::ProviderKind;
    use crate::util::SECS_PER_DAY;

    fn now() -> u64 {
        BUILT + SECS_PER_DAY
    }

    fn running_state(target: Target) -> RunState {
        let mut state = RunState::new(
            target,
            ProviderKind::Qemu,
            format!("/srv/vm/run/{target}/overlay.qcow2").into(),
            StartReason::Keep,
            BUILT,
        );
        state.ssh_host = "127.0.0.1".to_owned();
        state.ssh_port = 2222;
        state.ssh_user = "tester".to_owned();
        state.vnc = Some("127.0.0.1:5900".to_owned());
        state.pid = Some(1234);
        state
    }

    #[test]
    fn nothing_built_says_so_and_points_at_the_build_command() {
        let mut inv = inventory(vec![empty(Target::Windows), empty(Target::Linux)]);
        inv.root_exists = false;
        let text = render(&inv, now());
        assert!(text.contains("windows: image missing"), "{text}");
        assert!(text.contains("linux: image missing"), "{text}");
        assert!(text.contains("does not exist yet"), "{text}");
        assert!(text.contains("installation media: none cached"), "{text}");
        assert!(text.contains("total: 0 B"), "{text}");
        // With nothing to free, neither cleanup command is advertised.
        assert!(!text.contains("--purge"), "{text}");
    }

    #[test]
    fn built_images_report_size_build_time_and_footprint() {
        let mut inv = inventory(vec![healthy(Target::Windows), healthy(Target::Linux)]);
        inv.iso = vec![FileInfo::new(
            "/srv/vm/iso/windows11-enterprise-eval.iso",
            7_092_807_680,
            BUILT,
        )];
        let text = render(&inv, now());
        assert!(text.contains("windows: image ok"), "{text}");
        assert!(text.contains("golden.qcow2  20.0 GiB"), "{text}");
        assert!(text.contains("built 1972-09-27T00:00:00Z"), "{text}");
        assert!(text.contains("windows11-enterprise-eval.iso"), "{text}");
        assert!(text.contains("total: 46.6 GiB"), "{text}");
        assert!(text.contains("destroy all --purge"), "{text}");
    }

    #[test]
    fn the_windows_image_reports_its_evaluation_age() {
        let inv = inventory(vec![healthy(Target::Windows), empty(Target::Linux)]);
        let text = render(&inv, BUILT + 80 * SECS_PER_DAY);
        assert!(text.contains("evaluation day 80 of 90"), "{text}");
        assert!(text.contains("expiring"), "{text}");
    }

    #[test]
    fn a_running_vm_carries_the_ssh_view_and_destroy_hints() {
        let mut entry = healthy(Target::Linux);
        entry.state = Some(running_state(Target::Linux));
        entry.running = Some(true);
        entry.run_files = vec![FileInfo::new(
            "/srv/vm/run/linux/overlay.qcow2",
            2 * 1024 * 1024 * 1024,
            BUILT,
        )];
        let text = render(&inventory(vec![empty(Target::Windows), entry]), now());
        assert!(text.contains("sunlit-e2e-linux is running"), "{text}");
        assert!(text.contains("kept after a test run (--keep)"), "{text}");
        assert!(text.contains("cargo xtask vm ssh linux"), "{text}");
        assert!(text.contains("cargo xtask vm view linux"), "{text}");
        assert!(text.contains("cargo xtask vm destroy linux"), "{text}");
        assert!(text.contains("golden image is untouched"), "{text}");
        assert!(text.contains("vnc 127.0.0.1:5900"), "{text}");
        assert!(text.contains("2.0 GiB in run state"), "{text}");
    }

    #[test]
    fn an_orphan_from_a_crashed_run_is_reported_as_leftovers() {
        let mut entry = healthy(Target::Linux);
        let mut state = running_state(Target::Linux);
        state.reason = StartReason::Run;
        entry.state = Some(state);
        entry.running = Some(false);
        entry.run_files = vec![FileInfo::new(
            "/srv/vm/run/linux/overlay.qcow2",
            1024,
            BUILT,
        )];
        let text = render(&inventory(vec![empty(Target::Windows), entry]), now());
        assert!(text.contains("registered but not running"), "{text}");
        assert!(text.contains("left behind by a test run"), "{text}");
        assert!(text.contains("cargo xtask vm destroy linux"), "{text}");
    }

    #[test]
    fn a_broken_state_file_is_surfaced_rather_than_swallowed() {
        let mut entry = healthy(Target::Linux);
        entry.state_error = Some("malformed VM state file: expected value".to_owned());
        let text = render(&inventory(vec![entry]), now());
        assert!(text.contains("state file problem"), "{text}");
    }

    #[test]
    fn the_totals_split_what_each_cleanup_command_frees() {
        let mut entry = healthy(Target::Linux);
        entry.run_files = vec![FileInfo::new(
            "/srv/vm/run/linux/overlay.qcow2",
            1024 * 1024 * 1024,
            BUILT,
        )];
        let text = render(&inventory(vec![entry]), now());
        assert!(
            text.contains("20.0 GiB in golden images and media"),
            "{text}"
        );
        assert!(text.contains("1.0 GiB in run state"), "{text}");
        assert!(
            text.contains("`cargo xtask vm destroy all` frees the run state"),
            "{text}"
        );
    }
}
