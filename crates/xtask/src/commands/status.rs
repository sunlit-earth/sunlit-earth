//! `cargo xtask vm status`: what exists, what is running, and what it costs.
//!
//! Unelevated and read-only like the doctor. Plan decision 13 asks for one
//! place that answers "what is on my disk, what is running, and how do I get
//! rid of it", with the command to do so printed next to every number.

use std::fmt::Write as _;

use crate::provider::desktop::Desktop;
use crate::provider::target::Image;
use crate::store::inventory::{ImageInventory, Inventory};
use crate::store::state::{RunState, StartReason};
use crate::util::{format_bytes, format_unix_utc};

/// Render the whole inventory.
pub fn render(inventory: &Inventory, now_unix: u64) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "image store: {}", inventory.root.display());
    if !inventory.root_exists {
        let _ = writeln!(
            out,
            "  (does not exist yet; `cargo xtask vm build-image <image>` creates it)"
        );
    }
    let _ = writeln!(out);

    for image in Image::ALL {
        let entry = inventory.for_image(image);
        let _ = writeln!(
            out,
            "{}",
            image_section(image, entry, now_unix, &inventory.root)
        );
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
            let name = file.name();
            let _ = write!(out, "  {name}  {}", format_bytes(file.bytes));
            // The directory holds one file that is not media, and `--iso`
            // deletes it with the rest. Listing it unlabeled read as a third
            // ISO; leaving it out would show two files and delete three.
            if let Some(note) = crate::store::windows_media::media_note(&name) {
                let _ = write!(out, "  ({note})");
            }
            let _ = writeln!(out);
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "total: {}", format_bytes(inventory.total_bytes()));
    let images: u64 = inventory
        .images
        .iter()
        .map(|t| t.image_bytes() + t.build_bytes())
        .sum();
    let runs: u64 = inventory.images.iter().map(ImageInventory::run_bytes).sum();
    let _ = writeln!(
        out,
        "  {} in golden images and media, {} in run state",
        format_bytes(images + inventory.iso_bytes()),
        format_bytes(runs)
    );
    if runs > 0 {
        let _ = writeln!(
            out,
            "  `cargo xtask vm down all` frees the run state and leaves the images alone"
        );
    }
    if images + inventory.iso_bytes() > 0 {
        let _ = writeln!(
            out,
            "  `cargo xtask vm purge all` frees everything, including the images; \
             rebuilding costs one `vm build-image` per image"
        );
    }
    out
}

fn image_section(
    image: Image,
    entry: Option<&ImageInventory>,
    now_unix: u64,
    root: &std::path::Path,
) -> String {
    let mut out = String::new();
    let Some(entry) = entry else {
        let _ = writeln!(out, "{image}: not inspected");
        return out;
    };

    let condition = entry.condition(now_unix);
    let _ = writeln!(
        out,
        "{image}: {} ({})",
        condition.label(),
        condition.detail()
    );
    let _ = writeln!(out, "  {}", entry.image().label());
    // A layer is listed under the disk it is a differencing child of, because
    // the two stand or fall together: the parent cannot be rebuilt without
    // rebuilding this, and it cannot be purged without taking this with it.
    if let Some(parent) = entry.image().parent() {
        let _ = writeln!(out, "  a layer over the {parent} image");
    }

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

    let _ = write!(out, "{}", run_state_section(image, entry, root));

    let _ = writeln!(
        out,
        "  footprint: {} image, {} run state",
        format_bytes(entry.image_bytes()),
        format_bytes(entry.run_bytes())
    );
    out
}

/// The half of an image's section that describes overlays and VMs.
fn run_state_section(image: Image, entry: &ImageInventory, root: &std::path::Path) -> String {
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
        // Named the way it sits under the run directory, not by its file name
        // alone: what is listed there is what `vm down` deletes, and a run
        // directory has subdirectories in it.
        let run_dir = crate::store::Store::new(root).run_dir(image);
        for file in &entry.run_files {
            let name = file
                .path
                .strip_prefix(&run_dir)
                .map_or_else(|_| file.name(), |rest| rest.display().to_string());
            let _ = writeln!(out, "    {name}  {}", format_bytes(file.bytes));
        }
    }

    match (&entry.state, entry.running) {
        (Some(state), Some(true)) => {
            let _ = write!(out, "{}", running_vm(image, state));
        }
        (Some(state), Some(false)) => {
            let _ = writeln!(
                out,
                "  {} is registered but not running, left behind by {}",
                state.vm_name,
                state.reason.label()
            );
            let _ = writeln!(
                out,
                "    `cargo xtask vm down {image}` removes the leftovers"
            );
        }
        (Some(state), None) => {
            // "recorded by <label>" rather than "recorded (<label>)": the
            // labels carry a parenthesis of their own, and a line that reads
            // "recorded (an image build (vm build-image))" reads as a mistake.
            let _ = writeln!(
                out,
                "  {} is recorded by {}; liveness not checked",
                state.vm_name,
                state.reason.label()
            );
            let _ = writeln!(out, "    `cargo xtask vm down {image}` removes it");
        }
        (None, _) => {}
    }

    if let Some(error) = &entry.state_error {
        let _ = writeln!(out, "  state file problem: {error}");
    }
    out
}

fn running_vm(image: Image, state: &RunState) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "  {} is running, started {} ({})",
        state.vm_name,
        format_unix_utc(state.started_unix),
        state.reason.label()
    );
    // Named because a run's results are about whichever desktop it ran under,
    // and the flag that chose it belongs to the command that has already
    // finished by the time anybody reads this.
    if let Some(desktop) = state.desktop.as_deref() {
        let _ = writeln!(
            out,
            "    desktop session: {}",
            Desktop::parse(desktop).map_or_else(|| desktop.to_owned(), |d| d.label().to_owned())
        );
    }
    if state.ssh_port > 0 {
        let _ = writeln!(
            out,
            "    ssh:     `cargo xtask vm ssh {image}`  ({}:{})",
            state.ssh_host, state.ssh_port
        );
    }
    let _ = writeln!(
        out,
        "    desktop: `cargo xtask vm view {image}`{}",
        state
            .vnc
            .as_ref()
            .map(|vnc| format!("  (vnc {vnc})"))
            .unwrap_or_default()
    );
    // A build's disk is the image being installed, not a throwaway child of an
    // image, and calling it an overlay would say the opposite of what ending
    // one costs: the install is lost and starts again from the media.
    let _ = writeln!(
        out,
        "    down:    `cargo xtask vm down {image}`  ({})",
        if state.reason == StartReason::Build {
            "frees the memory and deletes the unfinished disk, so the install starts \
             over; any golden image already in the store is untouched"
        } else {
            "frees the memory and the overlay; the golden image is untouched"
        }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::target::Image;
    use crate::provider::target::ProviderKind;
    use crate::store::inventory::FileInfo;
    use crate::store::inventory::fixtures::{BUILT, empty, healthy, inventory};
    use crate::util::SECS_PER_DAY;

    fn now() -> u64 {
        BUILT + SECS_PER_DAY
    }

    fn running_state(image: Image) -> RunState {
        let mut state = RunState::new(
            image,
            ProviderKind::Qemu,
            format!("/srv/vm/run/{image}/overlay.qcow2").into(),
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

    /// What this lists is what `vm down` deletes, and a run directory has
    /// subdirectories in it, so a file named by its own name alone is a file
    /// nobody can find: that is how two source archives of eight megabytes sat
    /// in the store unaccounted for.
    #[test]
    fn a_run_file_is_listed_by_where_it_sits_under_the_run_directory() {
        let run = std::path::Path::new("/srv/vm/run/linux-builder");
        let mut entry = healthy(Image::LinuxBuilder);
        entry.run_files = vec![
            FileInfo::new(run.join("overlay.qcow2"), 3 * 1024 * 1024, BUILT),
            FileInfo::new(run.join("job").join("job.sh"), 200, BUILT),
        ];
        let text = render(&inventory(vec![entry]), now());
        assert!(text.contains("overlay.qcow2  3.0 MiB"), "{text}");
        let nested = std::path::Path::new("job").join("job.sh");
        assert!(text.contains(&nested.display().to_string()), "{text}");
    }

    #[test]
    fn nothing_built_says_so_and_points_at_the_build_command() {
        let mut inv = inventory(Image::ALL.into_iter().map(empty).collect());
        inv.root_exists = false;
        let text = render(&inv, now());
        for image in Image::ALL {
            assert!(text.contains(&format!("{image}: missing")), "{text}");
            assert!(text.contains(image.label()), "{text}");
        }
        // A layer is listed under the disk it is a child of, because the two
        // stand or fall together.
        assert!(text.contains("a layer over the windows image"), "{text}");
        assert!(text.contains("does not exist yet"), "{text}");
        assert!(text.contains("installation media: none cached"), "{text}");
        assert!(text.contains("total: 0 B"), "{text}");
        // With nothing to free, neither cleanup command is advertised.
        assert!(!text.contains("--purge"), "{text}");
    }

    #[test]
    fn built_images_report_size_build_time_and_footprint() {
        let mut inv = inventory(vec![healthy(Image::Windows), healthy(Image::Linux)]);
        inv.iso = vec![FileInfo::new(
            "/srv/vm/iso/windows11-enterprise-eval.iso",
            7_092_807_680,
            BUILT,
        )];
        let text = render(&inv, now());
        assert!(text.contains("windows: ok"), "{text}");
        assert!(text.contains("golden.qcow2  20.0 GiB"), "{text}");
        assert!(text.contains("built 1972-09-27T00:00:00Z"), "{text}");
        assert!(text.contains("windows11-enterprise-eval.iso"), "{text}");
        assert!(text.contains("total: 46.6 GiB"), "{text}");
        assert!(text.contains("vm purge all"), "{text}");
    }

    #[test]
    fn the_windows_image_reports_its_evaluation_age() {
        let inv = inventory(vec![healthy(Image::Windows), empty(Image::Linux)]);
        let text = render(&inv, BUILT + 80 * SECS_PER_DAY);
        assert!(text.contains("evaluation day 80 of 90"), "{text}");
        assert!(text.contains("expiring"), "{text}");
    }

    #[test]
    fn a_running_vm_carries_the_ssh_view_and_destroy_hints() {
        let mut entry = healthy(Image::Linux);
        entry.state = Some(running_state(Image::Linux));
        entry.running = Some(true);
        entry.run_files = vec![FileInfo::new(
            "/srv/vm/run/linux/overlay.qcow2",
            2 * 1024 * 1024 * 1024,
            BUILT,
        )];
        let text = render(&inventory(vec![empty(Image::Windows), entry]), now());
        assert!(text.contains("sunlit-e2e-linux is running"), "{text}");
        assert!(text.contains("a test run kept with --keep"), "{text}");
        assert!(text.contains("cargo xtask vm ssh linux"), "{text}");
        assert!(text.contains("cargo xtask vm view linux"), "{text}");
        assert!(text.contains("cargo xtask vm down linux"), "{text}");
        assert!(text.contains("golden image is untouched"), "{text}");
        // A test run boots a throwaway child of the image, and that is what the
        // teardown frees; only a build has an unfinished image of its own.
        assert!(text.contains("frees the memory and the overlay"), "{text}");
        assert!(text.contains("vnc 127.0.0.1:5900"), "{text}");
        assert!(text.contains("2.0 GiB in run state"), "{text}");
    }

    #[test]
    fn an_orphan_from_a_crashed_run_is_reported_as_leftovers() {
        let mut entry = healthy(Image::Linux);
        let mut state = running_state(Image::Linux);
        state.reason = StartReason::Run;
        entry.state = Some(state);
        entry.running = Some(false);
        entry.run_files = vec![FileInfo::new(
            "/srv/vm/run/linux/overlay.qcow2",
            1024,
            BUILT,
        )];
        let text = render(&inventory(vec![empty(Image::Windows), entry]), now());
        assert!(text.contains("registered but not running"), "{text}");
        assert!(text.contains("left behind by a test run"), "{text}");
        assert!(text.contains("cargo xtask vm down linux"), "{text}");
    }

    #[test]
    fn an_image_build_is_reported_as_one_whether_it_is_running_or_not() {
        // A build carries the same record every other guest does, so the only
        // thing that says it is a build is the reason. Both states matter: one
        // is a build in progress and the other is what a crash leaves.
        let mut entry = empty(Image::Windows);
        let mut state = running_state(Image::Windows);
        state.reason = StartReason::Build;
        state.ssh_port = 22;
        entry.state = Some(state);
        entry.running = Some(true);
        entry.run_files = vec![FileInfo::new(
            "/srv/vm/run/windows/build.vhdx",
            11 * 1024 * 1024 * 1024,
            BUILT,
        )];
        let text = render(&inventory(vec![entry.clone(), empty(Image::Linux)]), now());
        assert!(text.contains("sunlit-e2e-windows is running"), "{text}");
        assert!(text.contains("an image build (vm build-image)"), "{text}");
        assert!(text.contains("build.vhdx  11.0 GiB"), "{text}");
        assert!(text.contains("cargo xtask vm view windows"), "{text}");
        // The file a build's teardown deletes is the image being installed, so
        // the hint says that and not "the overlay".
        assert!(text.contains("deletes the unfinished disk"), "{text}");
        assert!(text.contains("the install starts over"), "{text}");
        assert!(!text.contains("and the overlay"), "{text}");

        entry.running = Some(false);
        let crashed = render(&inventory(vec![entry.clone(), empty(Image::Linux)]), now());
        assert!(crashed.contains("registered but not running"), "{crashed}");
        assert!(
            crashed.contains("left behind by an image build"),
            "{crashed}"
        );
        assert!(crashed.contains("cargo xtask vm down windows"), "{crashed}");

        // And the third state: a record nothing asked the provider about, which
        // is what `vm doctor` renders. The labels carry a parenthesis of their
        // own, so this line puts none around them.
        entry.running = None;
        let unchecked = render(&inventory(vec![entry, empty(Image::Linux)]), now());
        assert!(
            unchecked.contains("is recorded by an image build (vm build-image)"),
            "{unchecked}"
        );
        assert!(!unchecked.contains("))"), "{unchecked}");
    }

    #[test]
    fn both_windows_media_files_are_listed_and_counted() {
        // The install media and its prompt-free repack. Two files rather than
        // one is what doubles the media footprint, so both are named and both
        // are in the total. The record beside them is listed as well, because
        // `vm purge windows --iso` deletes it too, and it is labeled for what
        // it is rather than passing as a third ISO.
        let mut inv = inventory(vec![empty(Image::Windows), empty(Image::Linux)]);
        inv.iso = vec![
            FileInfo::new(
                "/srv/vm/iso/windows11-enterprise-eval-noprompt.iso",
                7_092_805_632,
                BUILT,
            ),
            FileInfo::new(
                format!(
                    "/srv/vm/iso/{}",
                    crate::store::windows_media::SOURCE_MARK_FILE
                ),
                84,
                BUILT,
            ),
            FileInfo::new(
                "/srv/vm/iso/windows11-enterprise-eval.iso",
                7_092_807_680,
                BUILT,
            ),
        ];
        let text = render(&inv, now());
        assert!(text.contains("installation media: 3 files"), "{text}");
        assert!(text.contains("windows11-enterprise-eval.iso"), "{text}");
        assert!(
            text.contains("windows11-enterprise-eval-noprompt.iso"),
            "{text}"
        );
        assert!(
            text.contains(".source.json  84 B  (the record of which download"),
            "{text}"
        );
        // And the ISOs themselves carry no note, so the label distinguishes.
        assert!(
            text.contains("windows11-enterprise-eval.iso  6.6 GiB\n"),
            "{text}"
        );
        assert!(text.contains("total: 13.2 GiB"), "{text}");
        assert!(text.contains("vm purge all"), "{text}");
    }

    #[test]
    fn a_broken_state_file_is_surfaced_rather_than_swallowed() {
        let mut entry = healthy(Image::Linux);
        entry.state_error = Some("malformed VM state file: expected value".to_owned());
        let text = render(&inventory(vec![entry]), now());
        assert!(text.contains("state file problem"), "{text}");
    }

    #[test]
    fn the_totals_split_what_each_cleanup_command_frees() {
        let mut entry = healthy(Image::Linux);
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
            text.contains("`cargo xtask vm down all` frees the run state"),
            "{text}"
        );
    }
}
