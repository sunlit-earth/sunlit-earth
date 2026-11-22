//! What the update thread found, on the three surfaces that show it.
//!
//! The update window, the tray's entry and the banner at the foot of the panel
//! all follow one `UpdateController`, which holds the last answer the thread
//! gave and the versions the user skipped and dismissed. The menu, the tray
//! and IPC reach it through a thread-local, as `autostart::ui` is reached,
//! because all three are wired before or apart from each other.
//!
//! A failed check is shown in the window it was asked from and nowhere else:
//! the tray and the banner keep showing the last answer that came through.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use sunlit_core::update::{
    Offer, Release, Request, UpdateEvent, UpdateHandle, UpdateStatus, asset_key,
};
use tracing::warn;

use crate::install::{self, InstallKind, UpdateHelp};
use crate::{Links, MainWindow, TrayIcon, UpdatePhase, UpdateRelease, UpdateWindow};

thread_local! {
    static CONTROLLER: RefCell<Option<UpdateController>> = const { RefCell::new(None) };
}

/// What the window is to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Showing {
    /// A check is out; `known` is the last answer, if there is one.
    Checking {
        known: Option<UpdateStatus>,
    },
    Answer(UpdateStatus),
}

/// What decides the window's instructions, which does not change in a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub help: UpdateHelp,
    /// The archive this build is updated from, `None` for a platform with none.
    pub asset_key: Option<String>,
}

impl Install {
    /// The running copy's.
    pub fn detect() -> Self {
        Self::of(
            install::kind(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            install::bundled(),
        )
    }

    pub fn of(kind: InstallKind, os: &str, arch: &str, bundled: bool) -> Self {
        Self {
            help: install::update_help(kind, os, bundled),
            asset_key: asset_key(os, arch, bundled),
        }
    }
}

/// "0.7.0, December 1, 2026", or the version alone for a release without a
/// date.
pub fn heading(release: &Release) -> String {
    match release.date {
        Some(date) => format!(
            "{}, {} {}, {}",
            release.version,
            date.month(),
            date.day(),
            date.year()
        ),
        None => release.version.to_string(),
    }
}

/// A size in decimal megabytes with one decimal, as the website shows it.
fn megabytes(bytes: u64) -> String {
    let tenths = (bytes + 50_000) / 100_000;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

/// The download button's label and the page it opens: the archive for this
/// build when the file has a usable one, the website's download section
/// otherwise.
pub fn download(offer: &Offer, key: Option<&str>, fallback: &str) -> (String, String) {
    let latest = offer.latest();
    let asset = key.and_then(|key| latest.assets.get(key));
    match asset.and_then(|asset| Some((asset.url.as_deref()?, asset.size))) {
        Some((url, size)) if crate::open::is_web_url(url) => {
            let label = match size {
                Some(size) => format!("Download {} ({})", latest.version, megabytes(size)),
                None => format!("Download {}", latest.version),
            };
            (label, url.to_owned())
        }
        _ => ("Open Download Page".to_owned(), fallback.to_owned()),
    }
}

fn offered(status: Option<&UpdateStatus>) -> Option<String> {
    match status {
        Some(UpdateStatus::Available(offer)) => Some(offer.latest().version.to_string()),
        _ => None,
    }
}

/// The version the tray's entry names: the one on offer unless it is skipped.
pub fn tray_version(status: Option<&UpdateStatus>, skipped: Option<&str>) -> String {
    offered(status)
        .filter(|version| Some(version.as_str()) != skipped)
        .unwrap_or_default()
}

/// The version the banner offers: the one on offer unless it is skipped or
/// dismissed.
pub fn banner_version(
    status: Option<&UpdateStatus>,
    skipped: Option<&str>,
    dismissed: Option<&str>,
) -> String {
    offered(status)
        .filter(|version| Some(version.as_str()) != skipped && Some(version.as_str()) != dismissed)
        .unwrap_or_default()
}

fn known_text(status: &UpdateStatus) -> String {
    match status {
        UpdateStatus::Available(offer) => format!(
            "Version {} was available at the last check.",
            offer.latest().version
        ),
        UpdateStatus::UpToDate | UpdateStatus::Failed(_) => String::new(),
    }
}

/// Put `showing` into `window`.
pub fn present(window: &UpdateWindow, showing: &Showing, install: &Install) {
    window.set_current_version(env!("CARGO_PKG_VERSION").into());
    let status = match showing {
        Showing::Checking { known } => {
            window.set_phase(UpdatePhase::Checking);
            window.set_known(known.as_ref().map_or_else(String::new, known_text).into());
            return;
        }
        Showing::Answer(status) => status,
    };
    match status {
        UpdateStatus::UpToDate => window.set_phase(UpdatePhase::UpToDate),
        UpdateStatus::Failed(reason) => {
            window.set_failure(reason.into());
            window.set_phase(UpdatePhase::Failed);
        }
        UpdateStatus::Available(offer) => {
            let latest = offer.latest();
            window.set_offered_version(latest.version.to_string().into());
            let releases: Vec<UpdateRelease> = offer
                .releases
                .iter()
                .map(|release| UpdateRelease {
                    heading: heading(release).into(),
                    highlights: ModelRc::new(VecModel::from(
                        release
                            .highlights
                            .iter()
                            .map(SharedString::from)
                            .collect::<Vec<_>>(),
                    )),
                })
                .collect();
            window.set_releases(ModelRc::new(VecModel::from(releases)));
            window.set_notes_url(
                latest
                    .notes
                    .as_deref()
                    .filter(|url| crate::open::is_web_url(url))
                    .unwrap_or_default()
                    .into(),
            );
            match &install.help {
                UpdateHelp::Command { command, hint } => {
                    window.set_command((*command).into());
                    window.set_command_hint((*hint).into());
                }
                UpdateHelp::Archive { steps } => {
                    let fallback = window.global::<Links>().get_download();
                    let (label, url) = download(offer, install.asset_key.as_deref(), &fallback);
                    window.set_command(SharedString::new());
                    window.set_download_label(label.into());
                    window.set_download_url(url.into());
                    window.set_archive_steps(ModelRc::new(VecModel::from(
                        steps
                            .iter()
                            .map(|step| SharedString::from(*step))
                            .collect::<Vec<_>>(),
                    )));
                }
            }
            window.set_phase(UpdatePhase::Available);
        }
    }
}

struct State {
    /// The last answer that was not a failure.
    known: Option<UpdateStatus>,
    skipped: Option<String>,
    dismissed: Option<String>,
    /// Whether a check the window asked for is still out.
    pending: bool,
    showing: Showing,
    window: Option<UpdateWindow>,
    tray: Option<slint::Weak<TrayIcon>>,
}

/// The three surfaces and the thread's requests, behind one handle.
#[derive(Clone)]
pub struct UpdateController {
    state: Rc<RefCell<State>>,
    main: slint::Weak<MainWindow>,
    install: Rc<Install>,
    requests: Rc<dyn Fn(Request)>,
}

impl UpdateController {
    /// A controller sending its requests to `requests`. Nothing is shown and
    /// nothing is wired until [`Self::wire`].
    pub fn new(main: &MainWindow, install: Install, requests: impl Fn(Request) + 'static) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                known: None,
                skipped: None,
                dismissed: None,
                pending: false,
                showing: Showing::Checking { known: None },
                window: None,
                tray: None,
            })),
            main: main.as_weak(),
            install: Rc::new(install),
            requests: Rc::new(requests),
        }
    }

    /// Wire the main window's entry, Details and close button.
    pub fn wire(&self, main: &MainWindow) {
        let this = self.clone();
        main.on_check_for_updates(move || this.open(true));
        let this = self.clone();
        main.on_show_update(move || this.open(false));
        let this = self.clone();
        main.on_dismiss_update_banner(move || this.dismiss_banner());
    }

    /// Have the tray's entry follow the answers and open the window.
    pub fn wire_tray(&self, tray: &TrayIcon) {
        self.state.borrow_mut().tray = Some(tray.as_weak());
        let this = self.clone();
        tray.on_show_update(move || {
            let nothing_on_offer = tray_version(
                this.state.borrow().known.as_ref(),
                this.state.borrow().skipped.as_deref(),
            )
            .is_empty();
            this.open(nothing_on_offer);
        });
        self.refresh_surfaces();
    }

    /// Whether the update window exists and is on screen, as a question that
    /// does not keep the window alive.
    pub fn visibility(&self) -> impl Fn() -> bool + 'static {
        let state = Rc::downgrade(&self.state);
        move || {
            state.upgrade().is_some_and(|state| {
                state
                    .borrow()
                    .window
                    .as_ref()
                    .is_some_and(|window| window.window().is_visible())
            })
        }
    }

    /// The update window, while it exists.
    pub fn window(&self) -> Option<UpdateWindow> {
        self.state
            .borrow()
            .window
            .as_ref()
            .map(ComponentHandle::clone_strong)
    }

    /// Take one report from the update thread.
    pub fn apply(&self, event: UpdateEvent) {
        if event.manual {
            crate::ipc::signal(&signal_line(&event.status));
        }
        {
            let mut state = self.state.borrow_mut();
            // Only this side sends a skip or a dismissal, so its own record is never
            // older than the thread's.
            state.skipped = state.skipped.take().or(event.skipped_version);
            state.dismissed = state.dismissed.take().or(event.dismissed_version);
            if !matches!(event.status, UpdateStatus::Failed(_)) {
                state.known = Some(event.status.clone());
            }
            if event.manual || !state.pending {
                state.pending = false;
                state.showing = Showing::Answer(event.status);
            } else {
                state.showing = Showing::Checking {
                    known: state.known.clone(),
                };
            }
        }
        self.redraw_window();
        if self.visibility()() {
            self.dismiss_shown_offer();
        }
        self.refresh_surfaces();
    }

    /// Open the window, checking first when `check` asks for it or nothing
    /// is known yet, and dismiss the banner for what it shows.
    pub fn open(&self, check: bool) {
        let (known, pending) = {
            let state = self.state.borrow();
            (state.known.clone(), state.pending)
        };
        if check || (known.is_none() && !pending) {
            {
                let mut state = self.state.borrow_mut();
                state.pending = true;
                state.showing = Showing::Checking { known };
            }
            (self.requests)(Request::CheckNow);
        } else if let Some(known) = known.filter(|_| !pending) {
            self.state.borrow_mut().showing = Showing::Answer(known);
        }
        if let Err(e) = self.ensure_window().and_then(|window| window.show()) {
            warn!("the update window could not be shown: {e}");
        }
        self.redraw_window();
        self.dismiss_shown_offer();
        self.refresh_surfaces();
    }

    /// Ask for a check without opening anything, as IPC does.
    pub fn check_now(&self) {
        (self.requests)(Request::CheckNow);
    }

    fn dismiss_banner(&self) {
        let version = banner_version(self.state.borrow().known.as_ref(), None, None);
        if !version.is_empty() {
            self.state.borrow_mut().dismissed = Some(version.clone());
            (self.requests)(Request::Dismiss(version));
        }
        self.refresh_surfaces();
    }

    /// Dismiss the banner for the version the window shows on offer.
    fn dismiss_shown_offer(&self) {
        let shown = match &self.state.borrow().showing {
            Showing::Answer(status) => offered(Some(status)),
            Showing::Checking { .. } => None,
        };
        if let Some(version) = shown
            && self.state.borrow().dismissed.as_deref() != Some(version.as_str())
        {
            self.state.borrow_mut().dismissed = Some(version.clone());
            (self.requests)(Request::Dismiss(version));
        }
    }

    fn skip(&self) {
        let shown = match &self.state.borrow().showing {
            Showing::Answer(status) => offered(Some(status)),
            Showing::Checking { .. } => None,
        };
        if let Some(version) = shown {
            self.state.borrow_mut().skipped = Some(version.clone());
            (self.requests)(Request::Skip(version));
        }
        self.hide_window();
        self.refresh_surfaces();
    }

    fn hide_window(&self) {
        if let Some(window) = self.window() {
            let _ = window.hide();
            crate::window_state::other_window_closed();
        }
    }

    fn ensure_window(&self) -> Result<UpdateWindow, slint::PlatformError> {
        if let Some(window) = self.window() {
            return Ok(window);
        }
        let window = UpdateWindow::new()?;
        window.on_open_url(|url| crate::open::url(&url));
        let this = self.clone();
        window.on_check_again(move || this.open(true));
        let this = self.clone();
        window.on_skip(move || this.skip());
        let this = self.clone();
        window.on_dismiss(move || this.hide_window());
        window.window().on_close_requested(|| {
            crate::window_state::other_window_closed();
            slint::CloseRequestResponse::HideWindow
        });
        self.state.borrow_mut().window = Some(window.clone_strong());
        Ok(window)
    }

    fn redraw_window(&self) {
        let state = self.state.borrow();
        if let Some(window) = state.window.as_ref() {
            present(window, &state.showing, &self.install);
        }
    }

    fn refresh_surfaces(&self) {
        let state = self.state.borrow();
        let skipped = state.skipped.as_deref();
        if let Some(main) = self.main.upgrade() {
            main.set_update_banner_version(
                banner_version(state.known.as_ref(), skipped, state.dismissed.as_deref()).into(),
            );
        }
        if let Some(tray) = state.tray.as_ref().and_then(slint::Weak::upgrade) {
            tray.set_update_version(tray_version(state.known.as_ref(), skipped).into());
        }
    }
}

/// `update state=<up_to_date|available|failed> version=<version or ->`.
pub fn signal_line(status: &UpdateStatus) -> String {
    let (state, version) = match status {
        UpdateStatus::UpToDate => ("up_to_date", "-".to_owned()),
        UpdateStatus::Available(offer) => ("available", offer.latest().version.to_string()),
        UpdateStatus::Failed(_) => ("failed", "-".to_owned()),
    };
    format!("update state={state} version={version}")
}

/// Start the update thread and wire the main window to it.
///
/// The thread's reports cross to the event loop through
/// `invoke_from_event_loop`; the handle lives in the thread-local until
/// [`shut_down`] drops it.
pub fn start(main: &MainWindow, enabled: bool) {
    let handle: Rc<RefCell<Option<UpdateHandle>>> = Rc::new(RefCell::new(None));
    let sender = Rc::clone(&handle);
    let controller = UpdateController::new(main, Install::detect(), move |request| {
        if let Some(handle) = sender.borrow().as_ref() {
            match request {
                Request::CheckNow => handle.check_now(),
                Request::Skip(version) => handle.skip(version),
                Request::Dismiss(version) => handle.dismiss(version),
            }
        }
    });
    controller.wire(main);
    *handle.borrow_mut() = Some(sunlit_core::update::spawn(
        enabled,
        Box::new(|event| {
            let posted = slint::invoke_from_event_loop(move || {
                CONTROLLER.with(|slot| {
                    if let Some(controller) = slot.borrow().as_ref() {
                        controller.apply(event);
                    }
                });
            });
            if let Err(e) = posted {
                warn!("an update report could not reach the event loop: {e}");
            }
        }),
    ));
    crate::window_state::register_other_window(controller.visibility());
    HANDLE.with(|slot| *slot.borrow_mut() = Some(handle));
    CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller));
}

/// Have the tray's entry follow the answers, once the tray exists.
pub fn register_tray(tray: &TrayIcon) {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow().as_ref() {
            controller.wire_tray(tray);
        }
    });
}

thread_local! {
    static HANDLE: RefCell<Option<Rc<RefCell<Option<UpdateHandle>>>>> = const { RefCell::new(None) };
}

/// Ask for a check at once, for IPC.
pub fn check_now() {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow().as_ref() {
            controller.check_now();
        }
    });
}

/// Drop the handle, which ends the thread at its next wake.
pub fn shut_down() {
    HANDLE.with(|slot| {
        if let Some(handle) = slot.borrow_mut().take() {
            handle.borrow_mut().take();
        }
    });
    CONTROLLER.with(|slot| slot.borrow_mut().take());
}

/// The releases the window lists, as headings with their highlights.
pub fn listed(window: &UpdateWindow) -> Vec<(String, Vec<String>)> {
    window
        .get_releases()
        .iter()
        .map(|release| {
            (
                release.heading.to_string(),
                release
                    .highlights
                    .iter()
                    .map(|line| line.to_string())
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sunlit_core::update::Asset;

    fn release(version: &str, assets: &[(&str, Option<&str>, Option<u64>)]) -> Release {
        Release {
            version: sunlit_core::update::Version::parse(version).unwrap(),
            date: None,
            notes: None,
            highlights: Vec::new(),
            assets: assets
                .iter()
                .map(|(key, url, size)| {
                    (
                        (*key).to_owned(),
                        Asset {
                            url: url.map(ToOwned::to_owned),
                            size: *size,
                        },
                    )
                })
                .collect(),
        }
    }

    const URL: &str = "https://github.com/sunlit-earth/sunlit-earth/releases/download/v0.7.0/a.zip";

    #[test]
    fn the_download_names_the_version_and_the_size() {
        let offer = Offer {
            releases: vec![release(
                "0.7.0",
                &[("windows-x86_64", Some(URL), Some(35_800_000))],
            )],
        };
        assert_eq!(
            download(&offer, Some("windows-x86_64"), "fallback"),
            ("Download 0.7.0 (35.8 MB)".to_owned(), URL.to_owned())
        );
    }

    #[test]
    fn without_a_usable_asset_the_button_opens_the_download_page() {
        let offer = Offer {
            releases: vec![release(
                "0.7.0",
                &[
                    ("linux-x86_64", None, Some(1)),
                    ("windows-x86_64", Some(URL), None),
                ],
            )],
        };
        for key in [Some("linux-x86_64"), Some("linux-aarch64"), None] {
            assert_eq!(
                download(&offer, key, "https://sunlit.earth/#download"),
                (
                    "Open Download Page".to_owned(),
                    "https://sunlit.earth/#download".to_owned()
                )
            );
        }
        assert_eq!(
            download(&offer, Some("windows-x86_64"), "x").0,
            "Download 0.7.0"
        );
    }

    #[test]
    fn sizes_round_to_a_tenth_of_a_megabyte() {
        assert_eq!(megabytes(35_800_000), "35.8 MB");
        assert_eq!(megabytes(35_849_999), "35.8 MB");
        assert_eq!(megabytes(35_850_000), "35.9 MB");
        assert_eq!(megabytes(0), "0.0 MB");
    }

    #[test]
    fn a_heading_carries_the_date_in_words() {
        let mut dated = release("0.7.0", &[]);
        dated.date = Some(time::macros::date!(2026 - 12 - 01));
        assert_eq!(heading(&dated), "0.7.0, December 1, 2026");
        assert_eq!(heading(&release("0.6.1", &[])), "0.6.1");
    }

    #[test]
    fn a_skipped_version_leaves_the_tray_and_a_dismissed_one_only_the_banner() {
        let offer = UpdateStatus::Available(Offer {
            releases: vec![release("0.7.0", &[])],
        });
        assert_eq!(tray_version(Some(&offer), None), "0.7.0");
        assert_eq!(tray_version(Some(&offer), Some("0.7.0")), "");
        assert_eq!(tray_version(Some(&offer), Some("0.6.0")), "0.7.0");
        assert_eq!(banner_version(Some(&offer), None, Some("0.7.0")), "");
        assert_eq!(banner_version(Some(&offer), None, Some("0.6.0")), "0.7.0");
        assert_eq!(banner_version(Some(&offer), Some("0.7.0"), None), "");
        assert_eq!(tray_version(Some(&UpdateStatus::UpToDate), None), "");
        assert_eq!(tray_version(None, None), "");
    }

    #[test]
    fn the_signal_names_the_state_and_the_version() {
        let offer = UpdateStatus::Available(Offer {
            releases: vec![release("0.7.0", &[])],
        });
        assert_eq!(signal_line(&offer), "update state=available version=0.7.0");
        assert_eq!(
            signal_line(&UpdateStatus::UpToDate),
            "update state=up_to_date version=-"
        );
        assert_eq!(
            signal_line(&UpdateStatus::Failed("x".to_owned())),
            "update state=failed version=-"
        );
    }
}
