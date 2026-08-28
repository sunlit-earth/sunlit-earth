//! The state file that sits next to a VM's overlay.
//!
//! Plan decision 13: everything the xtask creates has to be findable again
//! after the orchestrator that created it is gone. A crashed run, a `--keep`
//! run, and a `vm up` all leave the same small file, so `vm status` and
//! `vm down` work from a record on disk rather than by scanning the host for
//! things that might be ours.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::provider::target::{Image, ProviderKind, Target, VM_NAME_PREFIX};

pub const STATE_VERSION: u32 = 1;

/// Why a VM exists, which is what `vm status` reports and what tells a reader
/// whether something was left behind deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartReason {
    /// A test run currently in progress.
    Run,
    /// A test run that finished and was kept with `--keep`.
    Keep,
    /// An interactive guest from `vm up`.
    Up,
    /// The guest a `vm build-image` is installing Windows into, or provisioning
    /// a layer in.
    ///
    /// It carries the same name and record as any other, so `vm status`,
    /// `vm view`, `vm ssh`, `vm down`, and the one-VM-at-a-time rule all apply
    /// to a build without new plumbing, and a build left behind by a crash is
    /// something the inventory can name rather than an orphan (amendment
    /// decision 17).
    Build,
    /// The guest a `cargo xtask dist` is building a release binary in, or
    /// verifying one in.
    Dist,
}

impl StartReason {
    /// What to call this in a report, with its article: the labels are read
    /// inside sentences such as "left behind by ...", and a sentence that
    /// supplies the article cannot fit all four.
    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "a test run",
            Self::Keep => "a run kept with --keep",
            Self::Up => "an interactive guest (vm up)",
            Self::Build => "an image build (vm build-image)",
            Self::Dist => "a release build (xtask dist)",
        }
    }

    /// What ending this guest costs beyond its own overlay, as a clause, or
    /// `None` when ending it costs nothing worth saying.
    ///
    /// Every guest the xtask boots holds nothing worth keeping, which is plan
    /// decision 14's whole lifecycle, so there is one answer here for one
    /// reason: a build holds an install of tens of minutes, and being told to
    /// end one, or ending one on the way to something else, without being told
    /// that is how an hour goes missing.
    ///
    /// It lives on the reason rather than at either call site because both the
    /// one-VM-at-a-time refusal and the teardown that is about to stop the guest
    /// have to say the same thing, and a sentence written twice is a sentence
    /// that drifts.
    ///
    /// A release build costs less than an image build and more than nothing: the
    /// compile starts again from an empty `target/`, which is tens of minutes,
    /// and no image is lost.
    pub fn cost_of_ending(self) -> Option<&'static str> {
        match self {
            Self::Build => Some(
                "ends the image build running in it and starts that install over \
                 from the media",
            ),
            Self::Dist => Some(
                "ends the release build running in it and starts that compile over \
                 from an empty target directory",
            ),
            Self::Run | Self::Keep | Self::Up => None,
        }
    }
}

/// What one running or left-behind VM is, and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    #[serde(default)]
    pub format_version: u32,
    /// The image this guest is a throwaway child of, as
    /// [`Image::slug`](crate::provider::target::Image::slug).
    ///
    /// Defaulted rather than required, because records written before there was
    /// more than one image per target carry no such field, and one of those is
    /// what `vm status` reads after an upgrade. An empty value reads as the
    /// desktop image of the recorded target, which is what those records are
    /// about: there was nothing else to boot.
    #[serde(default)]
    pub image: String,
    pub target: String,
    pub provider: String,
    pub vm_name: String,
    pub overlay: PathBuf,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: String,
    /// The always-on localhost VNC address of a QEMU guest, if any.
    #[serde(default)]
    pub vnc: Option<String>,
    #[serde(default)]
    pub qmp_port: Option<u16>,
    /// The QEMU process id. `Hyper-V` guests have none: the VM is owned by the
    /// hypervisor, not by a process of ours.
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub started_unix: u64,
    pub reason: StartReason,
    /// Whether this guest has been handed to a person, confirmed by the guest
    /// itself: on Windows an account with no password and Remote Desktop
    /// Services running, so `vmconnect` can open an enhanced session.
    ///
    /// Separate from the reason, because the two answer different questions and
    /// only one of them can be known in advance. The reason says why the guest
    /// exists; this says what was done to it, which is what `vm view` has to
    /// consult before telling somebody what the console will ask them for. A
    /// hand-over that failed leaves this false and the guest offering the basic
    /// session it always had.
    #[serde(default)]
    pub handed_over: bool,
    /// Which desktop this guest was booted into, as
    /// [`Desktop::flag`](crate::provider::desktop::Desktop::flag).
    ///
    /// Recorded rather than derived, and stored rather than passed, because the
    /// choice is made by the command that boots the guest and read twice
    /// afterwards by things that run later: the provider, which builds the
    /// `fw_cfg` argument from it on every start, and `vm status`, which is a
    /// separate process and has nowhere else to learn it from. `None` is a guest
    /// left on the image's own default, which is every Windows guest and every
    /// Linux one booted before this existed.
    #[serde(default)]
    pub desktop: Option<String>,
}

impl RunState {
    pub fn new(
        image: Image,
        provider: ProviderKind,
        overlay: PathBuf,
        reason: StartReason,
        started_unix: u64,
    ) -> Self {
        Self {
            format_version: STATE_VERSION,
            image: image.slug().to_owned(),
            target: image.target().slug().to_owned(),
            provider: provider.name().to_owned(),
            vm_name: image.vm_name(),
            overlay,
            ssh_host: String::new(),
            ssh_port: 0,
            ssh_user: String::new(),
            vnc: None,
            qmp_port: None,
            pid: None,
            started_unix,
            reason,
            handed_over: false,
            desktop: None,
        }
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("malformed VM state file: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn provider_kind(&self) -> Option<ProviderKind> {
        ProviderKind::parse(&self.provider)
    }

    /// The operating system this record is about, if it names one this xtask
    /// knows.
    ///
    /// `None` for a hand-edited or corrupted file. Callers that only need it to
    /// choose a sentence supply their own fallback rather than panicking on a
    /// file they were reading in order to explain something.
    pub fn target(&self) -> Option<Target> {
        Target::parse(&self.target)
    }

    /// The image this guest is a child of.
    ///
    /// A record with no image field is one written when there was one image per
    /// target, so it can only be about that target's desktop image. A record
    /// naming an image whose target disagrees with its own `target` field is
    /// rejected rather than reconciled: the two would send a teardown to two
    /// different places.
    pub fn image(&self) -> Option<Image> {
        if self.image.trim().is_empty() {
            return self.target().map(Image::desktop);
        }
        Image::parse(&self.image).filter(|image| match self.target() {
            Some(target) => image.target() == target,
            None => false,
        })
    }

    /// Whether this record describes something the xtask created.
    ///
    /// A teardown refuses to act on anything that fails this, so a
    /// hand-edited or corrupted state file cannot aim the teardown at another
    /// VM on the host.
    pub fn is_ours(&self) -> bool {
        self.vm_name.starts_with(VM_NAME_PREFIX) && self.image().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RunState {
        let mut state = RunState::new(
            Image::Linux,
            ProviderKind::Qemu,
            PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Keep,
            1_755_600_000,
        );
        state.ssh_host = "127.0.0.1".to_owned();
        state.ssh_port = 2222;
        state.ssh_user = "tester".to_owned();
        state.vnc = Some("127.0.0.1:5900".to_owned());
        state.qmp_port = Some(4444);
        state.pid = Some(1234);
        state
    }

    #[test]
    fn a_state_file_round_trips() {
        let state = sample();
        let parsed = RunState::from_json(&state.to_json()).expect("round trip");
        assert_eq!(parsed, state);
        assert_eq!(parsed.provider_kind(), Some(ProviderKind::Qemu));
    }

    #[test]
    fn the_reason_is_stored_in_lowercase() {
        let json = sample().to_json();
        assert!(json.contains(r#""reason": "keep""#), "{json}");
        let parsed = RunState::from_json(&json.replace(r#""keep""#, r#""up""#)).expect("parses");
        assert_eq!(parsed.reason, StartReason::Up);
    }

    #[test]
    fn a_minimal_state_file_parses() {
        let parsed = RunState::from_json(
            r#"{"target":"windows","provider":"hyperv","vm_name":"sunlit-e2e-windows",
                "overlay":"C:/vm/run/windows/overlay.vhdx","reason":"run"}"#,
        )
        .expect("defaults fill the rest");
        assert!(parsed.is_ours());
        assert_eq!(parsed.pid, None);
        assert_eq!(parsed.ssh_port, 0);
        // A record written before the hand-over was recorded at all reads as a
        // guest nobody was handed, which is the answer that costs a dialog
        // rather than the one that promises a password nobody blanked.
        assert!(!parsed.handed_over);
        // And one written before the desktop was recorded reads as a guest on
        // the image's own default, which is what it is.
        assert_eq!(parsed.desktop, None);
        // A record from before there was more than one image per target can
        // only be about that target's desktop image, and reads as one.
        assert_eq!(parsed.image(), Some(Image::Windows));
    }

    #[test]
    fn a_builder_guest_records_the_image_it_is_a_child_of() {
        let state = RunState::new(
            Image::WindowsBuilder,
            ProviderKind::HyperV,
            PathBuf::from("C:/vm/run/windows-builder/overlay.vhdx"),
            StartReason::Dist,
            1_755_600_000,
        );
        let json = state.to_json();
        assert!(json.contains(r#""image": "windows-builder""#), "{json}");
        // The target is the operating system, which is what the guest contract,
        // the SSH account and the job scripts key on.
        assert!(json.contains(r#""target": "windows""#), "{json}");
        assert!(json.contains(r#""reason": "dist""#), "{json}");
        let parsed = RunState::from_json(&json).expect("round trip");
        assert_eq!(parsed, state);
        assert_eq!(parsed.image(), Some(Image::WindowsBuilder));
        assert_eq!(parsed.target(), Some(Target::Windows));
        assert!(parsed.is_ours());
    }

    /// The image and the target are written together and read separately, so a
    /// record where they disagree would send a teardown to one image's overlay
    /// while asking the other's guest contract about it. Neither half wins.
    #[test]
    fn a_record_whose_image_and_target_disagree_is_not_ours() {
        let mut state = sample();
        state.image = Image::WindowsBuilder.slug().to_owned();
        assert_eq!(state.image(), None);
        assert!(!state.is_ours());

        let mut unknown = sample();
        unknown.image = "windows-server-core".to_owned();
        assert_eq!(unknown.image(), None);
        assert!(!unknown.is_ours());
    }

    #[test]
    fn a_guest_booted_into_a_named_desktop_says_which_in_its_record() {
        let mut state = sample();
        state.desktop = Some(crate::provider::desktop::Desktop::Xfce.flag().to_owned());
        let json = state.to_json();
        assert!(json.contains(r#""desktop": "xfce""#), "{json}");
        let parsed = RunState::from_json(&json).expect("round trip");
        assert_eq!(parsed, state);
        assert_eq!(
            parsed
                .desktop
                .as_deref()
                .and_then(crate::provider::desktop::Desktop::parse),
            Some(crate::provider::desktop::Desktop::Xfce)
        );
    }

    #[test]
    fn a_handed_over_guest_says_so_in_its_record() {
        let mut state = sample();
        state.handed_over = true;
        let json = state.to_json();
        assert!(json.contains(r#""handed_over": true"#), "{json}");
        assert_eq!(RunState::from_json(&json).expect("round trip"), state);
    }

    #[test]
    fn a_crashed_build_leaves_a_record_that_names_itself_as_one() {
        // Written before the image field existed, which is the shape a record
        // from the last release has.
        // What `vm status` reads after a build died: the same file every other
        // guest leaves, distinguishable only by its reason, and naming the
        // unfinished disk that `vm down` deletes with it.
        let json = r#"{"format_version":1,"target":"windows","provider":"hyperv",
             "vm_name":"sunlit-e2e-windows",
             "overlay":"C:/vm/run/windows/build.vhdx","reason":"build",
             "ssh_user":"tester","ssh_port":22,"started_unix":1755600000}"#;
        let parsed = RunState::from_json(json).expect("a build record parses");
        assert_eq!(parsed.reason, StartReason::Build);
        assert!(parsed.is_ours());
        assert!(parsed.reason.label().contains("build"));
        assert!(
            parsed.overlay.to_string_lossy().ends_with("build.vhdx"),
            "{:?}",
            parsed.overlay
        );
        // And it round trips under the same spelling, because the file is
        // written by one version of this and read by another.
        let mut state = RunState::new(
            Image::Windows,
            ProviderKind::HyperV,
            PathBuf::from("C:/vm/run/windows/build.vhdx"),
            StartReason::Build,
            1_755_600_000,
        );
        state.ssh_user = "tester".to_owned();
        state.ssh_port = 22;
        assert!(state.to_json().contains(r#""reason": "build""#));
        assert_eq!(
            RunState::from_json(&state.to_json()).expect("round trip"),
            state
        );
    }

    #[test]
    fn a_state_file_naming_a_foreign_vm_is_not_ours() {
        let mut state = sample();
        state.vm_name = "production-db".to_owned();
        assert!(!state.is_ours());

        let mut other = sample();
        other.target = "solaris".to_owned();
        assert!(!other.is_ours());
    }

    #[test]
    fn a_truncated_state_file_is_an_error_not_a_default() {
        assert!(RunState::from_json("{\"target\":").is_err());
        assert!(RunState::from_json("{}").is_err());
    }
}
