//! The Rust toolchain this tree pins, read out of `rust-toolchain.toml`.
//!
//! Plan decision 7: a release build has to be able to say which compiler made
//! it, and the two builder images have to install that compiler rather than
//! whatever `stable` meant on the day their image was built. Both facts come
//! from the one file at the repository root, which rustup already reads, so the
//! version is spelled once: the channel reaches the Linux builder as a Packer
//! variable and the Windows layer as `toolchain.ps1`'s argument, and both jobs
//! install it by name.
//!
//! The parse is separate from the file so it can be tested against text, and
//! the channel is validated because it ends up on a command line inside a
//! guest. A build against an unreadable pin is refused rather than guessed at:
//! guessing is how a release binary comes out of a compiler nobody chose.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The file rustup reads, at the repository root.
pub const PINNED_FILE: &str = "rust-toolchain.toml";

/// What the repository pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    /// The channel as rustup spells it, `1.94.0` or `stable`.
    pub channel: String,
    /// The profile the pin asks for, `minimal` here, which is also what the
    /// guests install with.
    pub profile: String,
    pub components: Vec<String>,
}

impl Toolchain {
    /// The `rustup toolchain install` arguments a builder's job runs.
    ///
    /// Explicit rather than relying on rustup installing a missing toolchain by
    /// itself, which 1.28.0 removed and 1.28.1 restored behind a variable. It
    /// is a no-op when the image already carries the channel and a download
    /// when the repository has moved on since the image was built.
    pub fn install_args(&self) -> Vec<String> {
        vec![
            "toolchain".to_owned(),
            "install".to_owned(),
            self.channel.clone(),
            "--profile".to_owned(),
            "minimal".to_owned(),
        ]
    }
}

#[derive(Deserialize)]
struct File {
    toolchain: Section,
}

#[derive(Deserialize)]
struct Section {
    channel: String,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    components: Vec<String>,
}

/// Parse the contents of a `rust-toolchain.toml`.
pub fn parse(text: &str) -> Result<Toolchain, String> {
    let file: File =
        toml::from_str(text).map_err(|e| format!("malformed {PINNED_FILE}: {}", first_line(&e)))?;
    let channel = file.toolchain.channel.trim().to_owned();
    check_channel(&channel)?;
    Ok(Toolchain {
        channel,
        profile: file
            .toolchain
            .profile
            .unwrap_or_else(|| "default".to_owned()),
        components: file.toolchain.components,
    })
}

/// Read the pin from a repository root.
pub fn read(repo_root: &Path) -> Result<Toolchain, String> {
    let path = pinned_path(repo_root);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&text)
}

/// The pin this checkout carries.
pub fn pinned() -> Result<Toolchain, String> {
    read(&crate::store::repo_root())
}

pub fn pinned_path(repo_root: &Path) -> PathBuf {
    repo_root.join(PINNED_FILE)
}

/// Refuse a channel that is not a bare rustup channel name.
///
/// The value is interpolated into a command line in both guests, so this is the
/// one place that can keep a shell metacharacter out of it. It is also the
/// cheap check on a file somebody edited by hand: an empty channel would
/// otherwise reach a guest as a `rustup toolchain install` with no argument.
fn check_channel(channel: &str) -> Result<(), String> {
    if channel.is_empty() {
        return Err(format!("{PINNED_FILE} names no channel"));
    }
    if let Some(bad) = channel
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '-' | '_'))
    {
        return Err(format!(
            "{PINNED_FILE} channel {channel:?} contains {bad:?}, which is not part of a \
             rustup channel name; the channel is passed to a command line inside a guest"
        ));
    }
    Ok(())
}

/// `toml`'s errors carry a span diagram over several lines, which reads badly
/// inside a one-line refusal.
fn first_line(error: &toml::de::Error) -> String {
    error
        .message()
        .lines()
        .next()
        .unwrap_or("unparseable")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_parses_into_its_three_parts() {
        let pin = parse(
            r#"
            [toolchain]
            channel = "1.94.0"
            profile = "minimal"
            components = ["rustfmt", "clippy"]
            "#,
        )
        .expect("parses");
        assert_eq!(pin.channel, "1.94.0");
        assert_eq!(pin.profile, "minimal");
        assert_eq!(pin.components, ["rustfmt", "clippy"]);
    }

    #[test]
    fn a_channel_is_enough() {
        let pin = parse("[toolchain]\nchannel = \"stable\"\n").expect("parses");
        assert_eq!(pin.channel, "stable");
        assert_eq!(pin.profile, "default");
        assert!(pin.components.is_empty());
    }

    #[test]
    fn surrounding_whitespace_is_not_part_of_the_channel() {
        assert_eq!(
            parse("[toolchain]\nchannel = \" 1.94.0 \"\n")
                .expect("parses")
                .channel,
            "1.94.0"
        );
    }

    #[test]
    fn a_file_with_no_channel_is_refused_by_name() {
        let err = parse("[toolchain]\nprofile = \"minimal\"\n").unwrap_err();
        assert!(err.contains(PINNED_FILE), "{err}");
        let err = parse("channel = \"1.94.0\"\n").unwrap_err();
        assert!(err.contains(PINNED_FILE), "{err}");
        let err = parse("not toml at all {{").unwrap_err();
        assert!(err.contains("malformed"), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
    }

    #[test]
    fn a_channel_that_is_not_a_channel_name_is_refused() {
        // The value reaches a command line inside a guest, so this is the one
        // place that can stop a metacharacter from getting there.
        for bad in [
            "1.94.0; rm -rf /",
            "stable && whoami",
            "$(id)",
            "sta ble",
            "",
        ] {
            let text = format!("[toolchain]\nchannel = \"{bad}\"\n");
            assert!(parse(&text).is_err(), "accepted {bad:?}");
        }
        // And the shapes rustup actually uses all pass.
        for good in [
            "1.94.0",
            "stable",
            "beta",
            "nightly-2026-03-02",
            "1.94.0-x86_64-pc-windows-msvc",
        ] {
            let text = format!("[toolchain]\nchannel = \"{good}\"\n");
            assert_eq!(parse(&text).expect("parses").channel, good);
        }
    }

    #[test]
    fn the_install_arguments_name_the_channel_and_the_minimal_profile() {
        let pin = parse("[toolchain]\nchannel = \"1.94.0\"\n").expect("parses");
        assert_eq!(
            pin.install_args(),
            ["toolchain", "install", "1.94.0", "--profile", "minimal"]
        );
    }

    /// The committed file is what rustup reads and what both builders install,
    /// so a version this parser cannot read is a release build that cannot
    /// start. Reading the real file rather than a fixture is the point.
    #[test]
    fn the_committed_pin_is_readable_and_names_a_version() {
        let pin = pinned().expect("the repository pins a toolchain");
        assert!(
            pin.channel.starts_with(|c: char| c.is_ascii_digit()),
            "the pin should be a version rather than a moving channel, so a \
             release build names one compiler: {}",
            pin.channel
        );
        assert_eq!(pin.profile, "minimal");
        // The gates run both of these through the pinned toolchain.
        for component in ["rustfmt", "clippy"] {
            assert!(
                pin.components.iter().any(|c| c == component),
                "{component} is not in the pin, so the gates would install it \
                 by hand on every machine"
            );
        }
    }
}
