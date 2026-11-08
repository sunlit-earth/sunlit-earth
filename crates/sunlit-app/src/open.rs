//! Handing a URL or a folder to the platform's own handler.
//!
//! `std::process::Command` rather than `ShellExecuteW`, which would be the
//! idiomatic Win32 call and would cost a scoped `#[allow(unsafe_code)]` for a
//! hyperlink. Each platform's command is built by a function of its own, so a
//! test can read the program and the arguments without running anything.

use std::path::Path;
use std::process::Command;

/// Open a web page in the default browser.
///
/// Anything [`is_web_url`] refuses is logged and dropped. On Windows the
/// opener is `cmd /C start`, whose console window is suppressed through the
/// safe `creation_flags`.
///
/// :param url: the page to open
pub fn url(url: &str) {
    if !is_web_url(url) {
        tracing::warn!("refusing to open {url:?}: only http and https are opened");
        return;
    }
    if let Err(error) = spawn(url_command(url)) {
        tracing::warn!("could not open {url:?}: {error}");
    }
}

/// Open a folder in the platform's file manager, creating it first.
///
/// No shell is involved anywhere, so the path is one argument whatever it
/// holds and needs no check.
///
/// :param path: the folder to open
pub fn folder(path: &Path) {
    if let Err(error) = std::fs::create_dir_all(path) {
        tracing::warn!("could not create {}: {error}", path.display());
        return;
    }
    if let Err(error) = spawn(folder_command(path)) {
        tracing::warn!("could not open {}: {error}", path.display());
    }
}

fn spawn(mut command: Command) -> std::io::Result<()> {
    command.spawn().map(|_| ())
}

/// Whether a URL is one this program will hand to a shell.
///
/// Every URL the app opens is compiled into the binary, so this guards nothing
/// that is reachable today. It exists so that the argument-injection question
/// is unaskable rather than answerable by reading the documents, and that
/// takes more than a scheme check: the Windows opener goes through `cmd /C`,
/// which reparses its argument and reads `&`, `|`, `<`, `>`, `^` and `%` as
/// its own. So the rest of the URL has to be spelled out of a set that holds
/// nothing a shell looks at.
///
/// What that set admits and what it costs, exactly. A path, a fragment, a
/// port, userinfo and a query string of one parameter all pass, since `?` and
/// `=` are in. A second query parameter does not, because `&` is out. Neither
/// does a percent-escape, because `%` is out. `,` and `;` are out as well:
/// they are legal in a URL and `start` reads both as argument delimiters, so a
/// URL carrying one would be truncated there and the wrong page opened, which
/// is a worse failure than a refusal. No URL the app ships needs any of the
/// four, and the tests walk every one of them through this check.
pub fn is_web_url(url: &str) -> bool {
    /// URL characters that are not shell characters anywhere the opener runs.
    const SAFE: &str = "-._~:/?#[]@+='";

    match url.split_once("://") {
        Some((scheme, rest)) => {
            matches!(scheme, "http" | "https")
                && !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
        }
        None => false,
    }
}

#[cfg(windows)]
fn url_command(url: &str) -> Command {
    use std::os::windows::process::CommandExt as _;

    /// `CREATE_NO_WINDOW`. A bare `start` flashes a console otherwise.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // The empty argument is `start`'s title parameter: without it `start`
    // reads a quoted URL as the window title and opens nothing.
    let mut command = Command::new("cmd");
    command
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
fn folder_command(path: &Path) -> Command {
    let mut command = Command::new("explorer.exe");
    command.arg(path);
    command
}

#[cfg(target_os = "macos")]
fn url_command(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(target_os = "macos")]
fn folder_command(path: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg(path);
    command
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn url_command(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn folder_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use slint::ComponentHandle as _;

    use super::*;

    /// The program a command runs and its arguments, as text.
    fn parts(command: &Command) -> (String, Vec<String>) {
        (
            command.get_program().to_string_lossy().into_owned(),
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn only_http_and_https_urls_are_opened() {
        for allowed in [
            "http://example.invalid/x",
            "https://example.invalid",
            "https://spdx.org/licenses/MIT.html",
            "https://example.invalid:8443/a/b#c",
            "https://example.invalid/search?q=one",
        ] {
            assert!(is_web_url(allowed), "{allowed} should be opened");
        }
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:someone@example.invalid",
            "https://",
            "example.invalid",
            "",
            "https://example.invalid/a b",
            // What `cmd /C` would read as its own rather than as a URL.
            "https://example.invalid/x&calc",
            "https://example.invalid/x|calc",
            "https://example.invalid/x>out",
            "https://example.invalid/x^y",
            "https://example.invalid/%PATH%",
            "https://example.invalid/\"x\"",
            "https://example.invalid/$(x)",
            "https://example.invalid/x\ny",
            // Legal in a URL, and `start` reads both as argument delimiters.
            "https://example.invalid/a,b",
            "https://example.invalid/a;b",
        ] {
            assert!(!is_web_url(refused), "{refused} should be refused");
        }
    }

    /// Every URL in `Links` reaches the opener, or a menu entry would log a
    /// refusal instead of opening a page.
    #[test]
    fn every_link_the_menus_open_is_one_the_opener_accepts() {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::MainWindow::new().expect("main window");
        let links = window.global::<crate::Links>();
        for link in [
            links.get_website(),
            links.get_repository(),
            links.get_new_issue(),
        ] {
            assert!(is_web_url(&link), "{link} is refused by the opener");
        }
    }

    #[test]
    fn a_url_is_handed_to_the_platforms_opener_as_one_argument() {
        let (program, args) = parts(&url_command("https://example.invalid/a"));
        if cfg!(windows) {
            assert_eq!(program, "cmd");
            assert_eq!(args, ["/C", "start", "", "https://example.invalid/a"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(args, ["https://example.invalid/a"]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, ["https://example.invalid/a"]);
        }
    }

    #[test]
    fn a_folder_is_handed_to_the_file_manager_as_its_one_argument() {
        let path = Path::new("some folder/with & odd; characters");
        let command = folder_command(path);
        let expected = if cfg!(windows) {
            "explorer.exe"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(command.get_program(), OsStr::new(expected));
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args, [path.as_os_str()]);
    }
}
