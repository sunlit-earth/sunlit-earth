//! Getting the Windows evaluation ISO onto the host.
//!
//! Best effort with a clear manual fallback, which is what the plan's success
//! criterion 3 asks for. The download is a single pinned link followed through
//! its redirects, and anything that comes back too small to be an ISO is
//! treated as the error page it almost certainly is.

use std::path::{Path, PathBuf};

use crate::runner::{Cmd, Runner};
use crate::store::Store;
use crate::util::format_bytes;

/// The Evaluation Center download page, for the manual route.
pub const EVAL_PAGE: &str =
    "https://www.microsoft.com/en-us/evalcenter/download-windows-11-enterprise";

/// The pinned link for Windows 11 Enterprise evaluation, x64, en-us.
///
/// Resolved 2026-08-19 through `go.microsoft.com/fwlink` to `aka.ms/Win11E-ISO-25H2-en-us`
/// and on to a `software-static.download.prss.microsoft.com` path holding
/// build 26200.6584 (media stamp 250915-1905), 7092807680 bytes. No session,
/// no cookie, no token: the redirect chain resolves for an anonymous client.
///
/// Which of the three links to pin is a judgement call. The `prss` URL embeds
/// the build and dies at the next revision; the `aka.ms` alias embeds the
/// release and dies at the next feature update; the fwlink id is the most
/// durable of the three, and is the one pinned here. Microsoft documents none
/// of them, so this is observed behavior rather than a contract, and the manual
/// fallback exists because of that.
pub const EVAL_FWLINK: &str =
    "https://go.microsoft.com/fwlink/?linkid=2334167&clcid=0x409&culture=en-us&country=us";

/// Anything smaller than this is not a Windows ISO. The real one is about
/// 6.6 GiB; an expired link or a form page comes back in kilobytes.
pub const MIN_PLAUSIBLE_ISO_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Whether a downloaded file is plausibly the ISO rather than an error page.
pub fn plausible_iso(bytes: u64) -> bool {
    bytes >= MIN_PLAUSIBLE_ISO_BYTES
}

/// What to tell someone when the automated fetch did not work.
pub fn manual_instructions(destination: &Path) -> String {
    format!(
        "The Windows evaluation ISO could not be downloaded automatically.\n\n\
         Microsoft publishes it behind a registration form, and the direct links \
         are not documented, so this is expected to break from time to time.\n\n\
         Do this instead:\n  \
         1. Open {EVAL_PAGE}\n  \
         2. Download the Windows 11 Enterprise evaluation, 64-bit, English (United States)\n  \
         3. Save it as exactly this path:\n     {}\n  \
         4. Run `cargo xtask vm build-image windows` again\n\n\
         The build does not care which revision it is, but the autounattend file \
         in vm/windows/ was written against build 26200 (25H2).",
        destination.display()
    )
}

/// Make sure the ISO is in the store, downloading it if it is not.
pub fn ensure_iso(runner: &dyn Runner, store: &Store) -> Result<PathBuf, String> {
    let destination = store.windows_iso();
    if let Ok(meta) = std::fs::metadata(&destination) {
        if plausible_iso(meta.len()) {
            println!(
                "using the cached ISO at {} ({})",
                destination.display(),
                format_bytes(meta.len())
            );
            return Ok(destination);
        }
        println!(
            "the cached ISO is only {}, which is too small to be one; fetching it again",
            format_bytes(meta.len())
        );
        let _ = std::fs::remove_file(&destination);
    }

    if let Some(dir) = destination.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }

    let Some(command) = download_command(runner, EVAL_FWLINK, &destination) else {
        return Err(manual_instructions(&destination));
    };

    println!("downloading the Windows evaluation ISO, about 6.6 GiB");
    println!("  from {EVAL_FWLINK}");
    println!("  to   {}", destination.display());
    let code = runner
        .stream(&command)
        .map_err(|e| format!("cannot run the downloader: {e}"))?;

    let size = std::fs::metadata(&destination)
        .map(|m| m.len())
        .unwrap_or(0);
    if code != 0 || !plausible_iso(size) {
        let _ = std::fs::remove_file(&destination);
        return Err(manual_instructions(&destination));
    }
    Ok(destination)
}

/// Pick a downloader. `curl` ships with Windows 10 and later and with most
/// Linux distributions; `wget` covers the rest.
pub fn download_command(runner: &dyn Runner, url: &str, destination: &Path) -> Option<Cmd> {
    let out = destination.to_string_lossy().into_owned();
    if runner.which("curl").is_some() {
        return Some(Cmd::new("curl").args([
            "--location".to_owned(),
            "--fail".to_owned(),
            "--retry".to_owned(),
            "3".to_owned(),
            "--output".to_owned(),
            out,
            url.to_owned(),
        ]));
    }
    if runner.which("wget").is_some() {
        return Some(Cmd::new("wget").args(["--output-document".to_owned(), out, url.to_owned()]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    #[test]
    fn an_error_page_is_not_mistaken_for_an_iso() {
        assert!(!plausible_iso(0));
        assert!(!plausible_iso(4096));
        assert!(!plausible_iso(MIN_PLAUSIBLE_ISO_BYTES - 1));
        assert!(plausible_iso(MIN_PLAUSIBLE_ISO_BYTES));
        assert!(plausible_iso(7_092_807_680));
    }

    #[test]
    fn the_manual_instructions_name_the_page_and_the_exact_path() {
        let text = manual_instructions(Path::new(r"C:\vm\iso\windows11-enterprise-eval.iso"));
        assert!(text.contains(EVAL_PAGE), "{text}");
        assert!(
            text.contains(r"C:\vm\iso\windows11-enterprise-eval.iso"),
            "{text}"
        );
        assert!(text.contains("build-image windows"), "{text}");
    }

    #[test]
    fn curl_is_preferred_and_follows_redirects() {
        let runner = FakeRunner::new().with_tool("curl", "/usr/bin/curl");
        let cmd = download_command(&runner, EVAL_FWLINK, Path::new("/vm/iso/win.iso"))
            .expect("curl is available");
        assert_eq!(cmd.program, "curl");
        assert!(
            cmd.args.contains(&"--location".to_owned()),
            "{:?}",
            cmd.args
        );
        assert!(cmd.args.contains(&"--fail".to_owned()), "{:?}", cmd.args);
        assert_eq!(cmd.args.last().map(String::as_str), Some(EVAL_FWLINK));
    }

    #[test]
    fn wget_is_the_fallback_and_no_downloader_is_not_a_crash() {
        let runner = FakeRunner::new().with_tool("wget", "/usr/bin/wget");
        assert_eq!(
            download_command(&runner, EVAL_FWLINK, Path::new("/vm/iso/win.iso")).map(|c| c.program),
            Some("wget".to_owned())
        );
        let bare = FakeRunner::new();
        assert!(download_command(&bare, EVAL_FWLINK, Path::new("/vm/iso/win.iso")).is_none());
    }

    #[test]
    fn the_pinned_link_is_the_fwlink_rather_than_the_build_specific_one() {
        // The `prss` URL embeds a build number and dies at the next revision.
        assert!(EVAL_FWLINK.contains("go.microsoft.com/fwlink"));
        assert!(!EVAL_FWLINK.contains("prss.microsoft.com"));
    }
}
