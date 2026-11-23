use super::*;

const OWN: u32 = 100;

fn screen(name: &str, x: i64, y: i64, w: i64, h: i64) -> Screen {
    Screen {
        name: Some(name.to_owned()),
        rect: Rect::from_origin(x, y, w, h),
    }
}

fn two_screens() -> Vec<Screen> {
    vec![
        screen("left", 0, 0, 1920, 1080),
        screen("right", 1920, 0, 2560, 1440),
    ]
}

fn candidate(rect: Rect) -> Candidate {
    Candidate {
        application: Some("player".to_owned()),
        pid: Some(7),
        rect,
        full_screen: false,
    }
}

fn covered(coverage: &Coverage) -> Option<&Cover> {
    match coverage {
        Coverage::Covered(cover) => Some(cover),
        _ => None,
    }
}

#[test]
fn a_window_equal_to_a_monitor_covers_it() {
    let cover = first_cover(
        &[candidate(Rect::from_origin(0, 0, 1920, 1080))],
        &two_screens(),
    )
    .expect("covered");
    assert_eq!(cover.monitor.as_deref(), Some("left"));
    assert_eq!(cover.signal, Signal::Geometry);
    assert_eq!(cover.pid, Some(7));
}

#[test]
fn a_window_larger_than_a_monitor_covers_it() {
    let cover = first_cover(
        &[candidate(Rect::from_origin(-8, -8, 1936, 1096))],
        &two_screens(),
    )
    .expect("covered");
    assert_eq!(cover.monitor.as_deref(), Some("left"));
}

#[test]
fn a_window_one_pixel_short_on_any_edge_does_not_cover() {
    let full = Rect::from_origin(0, 0, 1920, 1080);
    let short = [
        Rect { left: 1, ..full },
        Rect { top: 1, ..full },
        Rect {
            right: full.right - 1,
            ..full
        },
        Rect {
            bottom: full.bottom - 1,
            ..full
        },
    ];
    for rect in short {
        assert_eq!(
            first_cover(&[candidate(rect)], &two_screens()),
            None,
            "{rect:?}"
        );
    }
}

#[test]
fn a_window_on_the_second_monitor_names_it() {
    let cover = first_cover(
        &[candidate(Rect::from_origin(1920, 0, 2560, 1440))],
        &two_screens(),
    )
    .expect("covered");
    assert_eq!(cover.monitor.as_deref(), Some("right"));
}

#[test]
fn a_window_spanning_both_monitors_covers() {
    let spanning = Rect::from_origin(0, 0, 4480, 1440);
    assert!(first_cover(&[candidate(spanning)], &two_screens()).is_some());
}

#[test]
fn a_full_screen_state_covers_at_any_size_on_the_screen_it_overlaps_most() {
    let mut window = candidate(Rect::from_origin(1800, 100, 800, 600));
    window.full_screen = true;
    let cover = first_cover(&[window], &two_screens()).expect("covered");
    assert_eq!(cover.signal, Signal::FullScreenState);
    assert_eq!(cover.monitor.as_deref(), Some("right"));
}

#[test]
fn no_screens_means_only_the_state_covers() {
    let mut window = candidate(Rect::from_origin(0, 0, 1920, 1080));
    assert_eq!(first_cover(&[window.clone()], &[]), None);
    window.full_screen = true;
    let cover = first_cover(&[window], &[]).expect("covered");
    assert_eq!(cover.monitor, None);
}

#[test]
fn an_empty_screen_is_covered_by_nothing() {
    let screens = [screen("zero", 0, 0, 0, 0)];
    assert_eq!(
        first_cover(&[candidate(Rect::from_origin(0, 0, 10, 10))], &screens),
        None
    );
}

fn windows_window(rect: Rect) -> WindowsWindow {
    WindowsWindow {
        pid: 7,
        class: "Chrome_WidgetWin_1".to_owned(),
        application: Some("chrome.exe".to_owned()),
        rect,
        visible: true,
        minimized: false,
        cloaked: false,
        tool_window: false,
        layered: false,
        transparent: false,
    }
}

fn windows_monitors() -> Vec<Screen> {
    vec![
        screen(r"\\.\DISPLAY1", 0, 0, 2560, 1440),
        screen(r"\\.\DISPLAY2", 2560, 0, 1920, 1080),
    ]
}

#[test]
fn a_borderless_window_on_windows_covers() {
    let window = windows_window(Rect::from_origin(2560, 0, 1920, 1080));
    let coverage = windows_coverage(
        &[window],
        &windows_monitors(),
        NotificationState::Other,
        OWN,
    );
    let cover = covered(&coverage).expect("covered");
    assert_eq!(cover.monitor.as_deref(), Some(r"\\.\DISPLAY2"));
    assert_eq!(cover.application.as_deref(), Some("chrome.exe"));
}

#[test]
fn each_windows_skip_rule_skips() {
    let full = Rect::from_origin(0, 0, 2560, 1440);
    let base = windows_window(full);
    let mut skipped: Vec<(&str, WindowsWindow)> = vec![
        (
            "hidden",
            WindowsWindow {
                visible: false,
                ..base.clone()
            },
        ),
        (
            "minimized",
            WindowsWindow {
                minimized: true,
                ..base.clone()
            },
        ),
        (
            "cloaked",
            WindowsWindow {
                cloaked: true,
                ..base.clone()
            },
        ),
        (
            "tool window",
            WindowsWindow {
                tool_window: true,
                ..base.clone()
            },
        ),
        (
            "transparent overlay",
            WindowsWindow {
                layered: true,
                transparent: true,
                ..base.clone()
            },
        ),
        (
            "this process",
            WindowsWindow {
                pid: OWN,
                ..base.clone()
            },
        ),
    ];
    for class in WINDOWS_SHELL_CLASSES {
        skipped.push((
            class,
            WindowsWindow {
                class: (*class).to_owned(),
                ..base.clone()
            },
        ));
    }
    for (why, window) in skipped {
        assert_eq!(
            windows_coverage(
                &[window],
                &windows_monitors(),
                NotificationState::Other,
                OWN
            ),
            Coverage::Clear,
            "{why}"
        );
    }
    let layered_only = WindowsWindow {
        layered: true,
        ..base
    };
    assert!(
        covered(&windows_coverage(
            &[layered_only],
            &windows_monitors(),
            NotificationState::Other,
            OWN
        ))
        .is_some()
    );
}

#[test]
fn a_maximized_window_short_of_the_taskbar_does_not_cover() {
    // A maximized window's rectangle runs past the work area by its resize
    // borders, and stops at a visible taskbar.
    let maximized = windows_window(Rect {
        left: -8,
        top: -8,
        right: 2568,
        bottom: 1400,
    });
    assert_eq!(
        windows_coverage(
            &[maximized],
            &windows_monitors(),
            NotificationState::Other,
            OWN
        ),
        Coverage::Clear
    );
}

#[test]
fn direct3d_and_presentation_cover_without_a_monitor() {
    for (state, signal) in [
        (
            NotificationState::ExclusiveDirect3D,
            Signal::ExclusiveDirect3D,
        ),
        (
            NotificationState::PresentationMode,
            Signal::PresentationMode,
        ),
    ] {
        let coverage = windows_coverage(&[], &windows_monitors(), state, OWN);
        let cover = covered(&coverage).expect("covered");
        assert_eq!(cover.signal, signal);
        assert_eq!(cover.monitor, None);
    }
}

#[test]
fn busy_alone_does_not_cover() {
    assert_eq!(
        windows_coverage(&[], &windows_monitors(), NotificationState::Busy, OWN),
        Coverage::Clear
    );
}

fn x11_window(rect: Rect) -> X11Window {
    X11Window {
        application: Some("vlc".to_owned()),
        pid: Some(4711),
        rect,
        viewable: true,
        hidden: false,
        full_screen: false,
        desktop: Some(0),
        kind: X11Type::Other,
    }
}

fn x11_monitors() -> Vec<Screen> {
    vec![
        screen("DP-1", 0, 0, 1920, 1080),
        screen("HDMI-1", 1920, 0, 1920, 1080),
    ]
}

#[test]
fn each_x11_skip_rule_skips() {
    let base = x11_window(Rect::from_origin(0, 0, 1920, 1080));
    let skipped = [
        (
            "hidden",
            X11Window {
                hidden: true,
                ..base.clone()
            },
        ),
        (
            "another desktop",
            X11Window {
                desktop: Some(1),
                ..base.clone()
            },
        ),
        (
            "desktop type",
            X11Window {
                kind: X11Type::Desktop,
                ..base.clone()
            },
        ),
        (
            "dock type",
            X11Window {
                kind: X11Type::Dock,
                ..base.clone()
            },
        ),
        (
            "unviewable",
            X11Window {
                viewable: false,
                ..base.clone()
            },
        ),
        (
            "this process",
            X11Window {
                pid: Some(OWN),
                ..base.clone()
            },
        ),
    ];
    for (why, window) in skipped {
        assert_eq!(
            x11_coverage(&[window], &[], &x11_monitors(), Some(0), OWN),
            Coverage::Clear,
            "{why}"
        );
    }
}

#[test]
fn an_x11_window_on_all_desktops_counts() {
    let window = X11Window {
        desktop: Some(X11_ALL_DESKTOPS),
        ..x11_window(Rect::from_origin(0, 0, 1920, 1080))
    };
    assert!(covered(&x11_coverage(&[window], &[], &x11_monitors(), Some(2), OWN)).is_some());
}

#[test]
fn the_x11_full_screen_state_covers_at_any_size() {
    let window = X11Window {
        full_screen: true,
        ..x11_window(Rect::from_origin(1920, 0, 640, 480))
    };
    let coverage = x11_coverage(&[window], &[], &x11_monitors(), Some(0), OWN);
    let cover = covered(&coverage).expect("covered");
    assert_eq!(cover.signal, Signal::FullScreenState);
    assert_eq!(cover.monitor.as_deref(), Some("HDMI-1"));
    assert_eq!(cover.application.as_deref(), Some("vlc"));
    assert_eq!(cover.pid, Some(4711));
}

#[test]
fn an_override_redirect_window_covers_by_geometry_alone() {
    let unmanaged = X11Window {
        application: None,
        pid: None,
        desktop: None,
        ..x11_window(Rect::from_origin(1920, 0, 1920, 1080))
    };
    let coverage = x11_coverage(
        &[],
        std::slice::from_ref(&unmanaged),
        &x11_monitors(),
        Some(0),
        OWN,
    );
    assert_eq!(covered(&coverage).map(|c| c.signal), Some(Signal::Geometry));

    let small = X11Window {
        full_screen: true,
        rect: Rect::from_origin(1920, 0, 100, 100),
        ..unmanaged.clone()
    };
    assert_eq!(
        x11_coverage(&[], &[small], &x11_monitors(), Some(0), OWN),
        Coverage::Clear
    );
    let unmapped = X11Window {
        viewable: false,
        ..unmanaged
    };
    assert_eq!(
        x11_coverage(&[], &[unmapped], &x11_monitors(), Some(0), OWN),
        Coverage::Clear
    );
}

#[test]
fn a_wlr_toplevel_covers_only_full_screen_on_an_output() {
    let both = WlrToplevel {
        app_id: Some("mpv".to_owned()),
        full_screen: true,
        outputs: vec!["DP-1".to_owned()],
    };
    let coverage = wlr_coverage(std::slice::from_ref(&both));
    let cover = covered(&coverage).expect("covered");
    assert_eq!(cover.monitor.as_deref(), Some("DP-1"));
    assert_eq!(cover.application.as_deref(), Some("mpv"));

    let no_output = WlrToplevel {
        outputs: Vec::new(),
        ..both.clone()
    };
    let not_full_screen = WlrToplevel {
        full_screen: false,
        ..both
    };
    assert_eq!(wlr_coverage(&[no_output, not_full_screen]), Coverage::Clear);
}

fn mac_displays() -> Vec<Screen> {
    vec![
        screen("1", 0, 0, 1512, 982),
        screen("2", 1512, 0, 1920, 1080),
    ]
}

fn mac_window(top: i64) -> MacWindow {
    MacWindow {
        pid: 7,
        layer: 0,
        bounds: Rect {
            left: 0,
            top,
            right: 1512,
            bottom: 982,
        },
    }
}

#[test]
fn a_mac_window_under_the_notch_tolerance_covers() {
    let coverage = mac_coverage(&[mac_window(NOTCH_TOLERANCE)], &mac_displays(), OWN);
    assert_eq!(
        covered(&coverage)
            .and_then(|c| c.monitor.clone())
            .as_deref(),
        Some("1")
    );
    assert!(covered(&mac_coverage(&[mac_window(0)], &mac_displays(), OWN)).is_some());
}

#[test]
fn a_mac_window_lower_than_the_tolerance_does_not_cover() {
    assert_eq!(
        mac_coverage(&[mac_window(NOTCH_TOLERANCE + 1)], &mac_displays(), OWN),
        Coverage::Clear
    );
}

#[test]
fn a_mac_window_off_layer_zero_or_of_this_process_does_not_cover() {
    let menu = MacWindow {
        layer: 24,
        ..mac_window(0)
    };
    let own = MacWindow {
        pid: OWN,
        ..mac_window(0)
    };
    assert_eq!(
        mac_coverage(&[menu, own], &mac_displays(), OWN),
        Coverage::Clear
    );
}

#[test]
fn the_cover_line_names_what_is_known() {
    let cover = Cover {
        application: Some("vlc".to_owned()),
        pid: Some(4711),
        monitor: Some("DP-1".to_owned()),
        signal: Signal::FullScreenState,
    };
    assert_eq!(
        Coverage::Covered(cover).to_string(),
        "vlc (pid 4711) on DP-1, full screen state"
    );
    assert_eq!(Coverage::Clear.to_string(), "none");
    assert_eq!(
        Coverage::Unknown("no route".to_owned()).to_string(),
        "unknown; no route"
    );
    assert_eq!(
        Cover::new(Signal::PresentationMode).to_string(),
        "an application, presentation mode"
    );
}
