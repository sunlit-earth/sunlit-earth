//! Reading `cargo --message-format=json`.
//!
//! Plan decision 8: the binaries are built on the host and copied into the
//! guest, so the orchestrator has to know where Cargo put them. Asking Cargo is
//! the only reliable answer; the hashed file name under `target/debug/deps` is
//! not something to reconstruct by hand.

use std::path::PathBuf;

use serde::Deserialize;

/// One built artifact Cargo reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub kinds: Vec<String>,
    pub executable: PathBuf,
    /// Whether it was built under the test profile, which is what separates
    /// the test harness from the application binary it drives.
    pub is_test: bool,
}

impl Artifact {
    pub fn has_kind(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == kind)
    }
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    target: RawTarget,
    #[serde(default)]
    profile: RawProfile,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    message: Option<RawDiagnostic>,
}

#[derive(Deserialize)]
struct RawDiagnostic {
    #[serde(default)]
    level: String,
    /// What `cargo build` would have printed, arrows and colours and all.
    #[serde(default)]
    rendered: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawTarget {
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Vec<String>,
}

#[derive(Deserialize, Default)]
struct RawProfile {
    #[serde(default)]
    test: bool,
}

/// Pull the executables out of a stream of Cargo messages.
///
/// Non-JSON lines are ignored rather than fatal: Cargo interleaves its own
/// progress output on stdout in some configurations, and a warning is not a
/// reason to lose the build.
pub fn parse_artifacts(stdout: &str) -> Vec<Artifact> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<RawMessage>(line.trim()).ok())
        .filter(|message| message.reason == "compiler-artifact")
        .filter_map(|message| {
            let executable = message.executable?;
            (!executable.is_empty()).then(|| Artifact {
                name: message.target.name,
                kinds: message.target.kind,
                executable: PathBuf::from(executable),
                is_test: message.profile.test,
            })
        })
        .collect()
}

/// What the compiler said, for a build that has just failed.
///
/// `--message-format=json` is what tells this crate where Cargo put the
/// binaries, and it moves the diagnostics with it: rustc's errors arrive as
/// `compiler-message` records on stdout, which is captured, while stderr keeps
/// only the summary. So a failed build printed "could not compile ... due to 1
/// previous error" over a stream that never said what the error was. This is the
/// half that has to be printed by hand, and `rendered` is the same text a plain
/// `cargo build` would have shown.
pub fn rendered_diagnostics(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<RawMessage>(line.trim()).ok())
        .filter(|message| message.reason == "compiler-message")
        .filter_map(|message| message.message)
        .filter(|diagnostic| matches!(diagnostic.level.as_str(), "error" | "warning"))
        .filter_map(|diagnostic| diagnostic.rendered)
        .filter(|rendered| !rendered.trim().is_empty())
        .collect()
}

/// The compiled test harness for one integration test target.
pub fn test_binary<'a>(artifacts: &'a [Artifact], name: &str) -> Option<&'a Artifact> {
    artifacts
        .iter()
        .find(|a| a.is_test && a.name == name && a.has_kind("test"))
}

/// The application binary.
///
/// It is built under the test profile too, as a dependency of the suite, and
/// it is the one the suite spawns, so this is the copy that has to go into the
/// guest.
pub fn bin<'a>(artifacts: &'a [Artifact], name: &str) -> Option<&'a Artifact> {
    artifacts
        .iter()
        .find(|a| a.name == name && a.has_kind("bin"))
}

/// A failed build has to say what the compiler said, and the compiler says it
/// on the stream this module is reading rather than on the terminal.
#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    const FAILED: &str = concat!(
        r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"serde"},"executable":null}"#,
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"warning","rendered":"warning: unused import"}}"#,
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"error","rendered":"error[E0063]: missing field `by_position`\n  --> src/lib.rs:5:9"}}"#,
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"failure-note","rendered":"note: a note nobody needs"}}"#,
        "\n",
        r#"{"reason":"build-finished","success":false}"#,
    );

    #[test]
    fn the_compilers_own_words_are_recovered_from_the_json() {
        let rendered = rendered_diagnostics(FAILED);
        assert_eq!(rendered.len(), 2, "{rendered:?}");
        assert!(rendered[0].contains("unused import"), "{rendered:?}");
        assert!(rendered[1].contains("E0063"), "{rendered:?}");
        // The whole rendering, arrows and all, rather than the message alone.
        assert!(rendered[1].contains("--> src/lib.rs:5:9"), "{rendered:?}");
    }

    #[test]
    fn a_build_that_said_nothing_yields_nothing_rather_than_an_empty_line() {
        assert!(rendered_diagnostics("").is_empty());
        assert!(rendered_diagnostics(r#"{"reason":"build-finished","success":true}"#).is_empty());
        let blank = r#"{"reason":"compiler-message","message":{"level":"error","rendered":"  "}}"#;
        assert!(rendered_diagnostics(blank).is_empty());
    }

    /// The artifact walk and the diagnostic walk read the same stream, and
    /// neither may be confused by the other's records.
    #[test]
    fn artifacts_and_diagnostics_do_not_see_each_other() {
        assert_eq!(parse_artifacts(FAILED), Vec::new());
        assert!(!rendered_diagnostics(FAILED).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = concat!(
        r#"{"reason":"compiler-artifact","package_id":"sunlit-core 0.1.0","target":{"kind":["lib"],"name":"sunlit-core"},"profile":{"test":false},"executable":null}"#,
        "\n",
        r#"{"reason":"compiler-artifact","package_id":"sunlit-earth 0.1.0","target":{"kind":["bin"],"name":"sunlit-earth"},"profile":{"test":false},"executable":"/home/dev/t/debug/sunlit-earth"}"#,
        "\n",
        r#"{"reason":"compiler-artifact","package_id":"sunlit-earth 0.1.0","target":{"kind":["test"],"name":"e2e"},"profile":{"test":true},"executable":"/home/dev/t/debug/deps/e2e-9f3c1a2b"}"#,
        "\n",
        r#"{"reason":"compiler-artifact","package_id":"sunlit-earth 0.1.0","target":{"kind":["test"],"name":"slint_ui"},"profile":{"test":true},"executable":"/home/dev/t/debug/deps/slint_ui-7d21"}"#,
        "\n",
        r#"{"reason":"build-finished","success":true}"#,
        "\n",
    );

    #[test]
    fn only_artifacts_with_an_executable_come_through() {
        let artifacts = parse_artifacts(STREAM);
        assert_eq!(artifacts.len(), 3);
        assert!(
            artifacts
                .iter()
                .all(|a| !a.executable.as_os_str().is_empty())
        );
    }

    #[test]
    fn the_test_harness_is_found_by_name() {
        let artifacts = parse_artifacts(STREAM);
        let e2e = test_binary(&artifacts, "e2e").expect("the e2e harness");
        assert_eq!(
            e2e.executable,
            PathBuf::from("/home/dev/t/debug/deps/e2e-9f3c1a2b")
        );
        assert!(e2e.is_test);
        assert!(test_binary(&artifacts, "soak").is_none());
    }

    #[test]
    fn the_application_binary_is_found_separately() {
        let artifacts = parse_artifacts(STREAM);
        let app = bin(&artifacts, "sunlit-earth").expect("the app");
        assert_eq!(
            app.executable,
            PathBuf::from("/home/dev/t/debug/sunlit-earth")
        );
        assert!(!app.is_test);
        // The harness and the binary it spawns are different files, and
        // confusing them would copy the wrong one into the guest.
        assert_ne!(
            app.executable,
            test_binary(&artifacts, "e2e").unwrap().executable
        );
    }

    #[test]
    fn a_lib_artifact_is_never_mistaken_for_a_binary() {
        let artifacts = parse_artifacts(STREAM);
        assert!(bin(&artifacts, "sunlit-core").is_none());
    }

    #[test]
    fn noise_on_the_stream_is_ignored_rather_than_fatal() {
        let noisy = format!("warning: unused variable\n{STREAM}not json either\n");
        assert_eq!(parse_artifacts(&noisy).len(), 3);
        assert!(parse_artifacts("").is_empty());
    }

    #[test]
    fn windows_paths_survive_the_round_trip() {
        let line = r#"{"reason":"compiler-artifact","target":{"kind":["test"],"name":"e2e"},"profile":{"test":true},"executable":"C:\\work\\target\\debug\\deps\\e2e-1a2b.exe"}"#;
        let artifacts = parse_artifacts(line);
        assert_eq!(
            artifacts[0].executable,
            PathBuf::from(r"C:\work\target\debug\deps\e2e-1a2b.exe")
        );
    }
}
