//! A syntax check for every `PowerShell` script this crate generates.
//!
//! Almost all of the Windows-side logic is a script built by string formatting
//! and handed to `powershell.exe`, and almost none of it can be executed here:
//! the setup steps change the machine and the `Hyper-V` ones need a hypervisor
//! and a group membership. What can be checked without running anything is
//! whether the scripts parse, which is exactly the class of mistake string
//! formatting produces. `Parser::ParseInput` does that: it is the parser
//! `PowerShell` itself uses, and it neither executes nor resolves anything.
//!
//! This found a real one on the way in: `Write-Output "STATE={0}" -f $x`, where
//! `-f` is a string operator rather than a parameter of `Write-Output`.

use crate::runner::{RealRunner, Runner};
#[cfg(windows)]
use crate::runner::{encode_command, powershell};

/// Every script the crate can produce, with a name to report it under.
#[cfg(windows)]
fn all_scripts() -> Vec<(String, String)> {
    use crate::commands::build_hyperv;
    use crate::commands::setup::{SetupInputs, windows_plan};
    use crate::host::facts::{FEATURE_HYPERV, WINDOWS_QEMU_DIRS};
    use crate::provider::hyperv;
    use crate::store::Store;

    let mut scripts = Vec::new();

    let probe = crate::host::facts::windows_probe_script(std::path::Path::new(r"C:\vm store"));
    scripts.push(("host probe".to_owned(), probe));

    let store = Store::new(r"C:\vm store");
    for step in windows_plan(&SetupInputs::default(), &store) {
        scripts.push((format!("setup: {}", step.name), step.script));
    }

    let name = "sunlit-e2e-windows";
    scripts.push((
        "hyperv: create".to_owned(),
        hyperv::create_script(
            name,
            r"C:\vm\golden.vhdx",
            r"C:\vm\overlay.vhdx",
            (1920, 1080),
            crate::provider::resources_for(crate::provider::target::Image::Windows),
        ),
    ));
    scripts.push(("hyperv: state".to_owned(), hyperv::state_script(name)));
    scripts.push(("hyperv: id".to_owned(), hyperv::id_script(name)));
    scripts.push(("hyperv: address".to_owned(), hyperv::address_script(name)));
    scripts.push(("hyperv: destroy".to_owned(), hyperv::destroy_script(name)));

    // A couple of one-liners the providers build inline.
    scripts.push((
        "hyperv: start".to_owned(),
        format!("Start-VM -Name {}", crate::runner::ps_quote(name)),
    ));

    // The native Windows image build: the VM it installs into, the signals it
    // watches, and the removal that keeps the disk.
    scripts.push((
        "build: create".to_owned(),
        build_hyperv::create_script(
            name,
            std::path::Path::new(r"C:\vm store\run\windows\build.vhdx"),
            std::path::Path::new(r"C:\vm store\iso\noprompt.iso"),
            std::path::Path::new(r"C:\vm store\build\windows\unattend.iso"),
            (1920, 1080),
            crate::provider::resources_for(crate::provider::target::Image::Windows),
        ),
    ));
    scripts.push((
        "hyperv: video".to_owned(),
        hyperv::video_script(name, (2560, 1440)),
    ));
    scripts.push(("hyperv: work area".to_owned(), hyperv::work_area_script()));
    scripts.push(("build: probe".to_owned(), build_hyperv::probe_script(name)));
    scripts.push((
        "build: remove".to_owned(),
        build_hyperv::remove_keeping_disk_script(name),
    ));

    // And the media repack, which mounts the installation media to copy it out.
    let iso = std::path::Path::new(r"C:\vm store\iso\windows11-enterprise-eval.iso");
    scripts.push((
        "media: mount".to_owned(),
        crate::store::windows_media::mount_script(iso),
    ));
    scripts.push((
        "media: dismount".to_owned(),
        crate::store::windows_media::dismount_script(iso),
    ));

    // The hand-over scripts, which are generated per boot and run in the guest.
    // A syntax error in either is invisible from the host: the shortcuts fail
    // as a warning and the enhanced session as a console that asks for a
    // password nobody blanked.
    scripts.push((
        "handover: shortcuts".to_owned(),
        crate::guest::handover::shortcut_script(&crate::guest::artifacts::guest_paths(
            crate::provider::target::Target::Windows,
            "sunlit-earth.exe",
            "e2e-1a2b.exe",
            true,
        )),
    ));
    scripts.push((
        "handover: enhanced session".to_owned(),
        crate::guest::handover::enhanced_session_script(hyperv::GUEST_USER),
    ));

    // The layer build's own commands, which run inside the guest over SSH.
    for script in [
        crate::commands::build_layer::TOOLCHAIN_SCRIPT,
        crate::commands::build_layer::FINALIZE_SCRIPT,
    ] {
        let leaf = std::path::Path::new(script)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        scripts.push((
            format!("layer: run {leaf}"),
            crate::commands::build_layer::script_command(
                &crate::commands::build_layer::script_destination(&leaf),
                "1.94.0",
            ),
        ));
    }

    // The scripts that ship in the repo and run inside the guest. A typo in
    // one of these surfaces forty minutes into a Windows image build, which is
    // the most expensive place in this phase to find one.
    //
    // Enumerated rather than listed: a list is a thing to forget to add to,
    // and the cost of forgetting is a script that is never checked.
    for path in windows_guest_scripts("ps1") {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        scripts.push((format!("guest: {name}"), text));
    }

    // Referenced so a rename here fails loudly rather than silently narrowing
    // what is checked.
    assert!(!FEATURE_HYPERV.is_empty() && !WINDOWS_QEMU_DIRS[0].is_empty());
    scripts
}

/// The guest-side scripts of one kind that ship under a Windows image's
/// template directory.
///
/// Every such directory, not the desktop image's alone: the layer ships its own
/// two scripts, and a list is a thing to forget to add to.
fn windows_guest_scripts(extension: &str) -> Vec<std::path::PathBuf> {
    template_scripts(crate::provider::target::Target::Windows, extension)
}

/// Every script of one kind under the template directories of the images of one
/// operating system.
fn template_scripts(
    target: crate::provider::target::Target,
    extension: &str,
) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for image in crate::provider::target::Image::ALL
        .into_iter()
        .filter(|image| image.target() == target)
    {
        let dir = crate::store::template_dir(image).join("scripts");
        if dir.is_dir() {
            out.extend(scripts_in(&dir, extension));
        }
    }
    out.sort();
    out
}

/// Every file with the given extension in a directory, sorted.
fn scripts_in(dir: &std::path::Path, extension: &str) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == extension))
        .collect();
    out.sort();
    out
}

/// Every shell script this crate generates for the Linux guest, with a name to
/// report it under.
///
/// The same class of mistake as the `PowerShell` half, and the same check for it:
/// these are built by string formatting, one of them carries three heredocs, and
/// a syntax error in either is invisible from the host. The hand-over fails as a
/// warning, and what it costs is a guest with no launcher in it.
fn generated_linux_scripts() -> Vec<(String, String)> {
    let paths = crate::guest::artifacts::guest_paths(
        crate::provider::target::Target::Linux,
        "sunlit-earth",
        "e2e-1a2b",
        true,
    );
    let bare = crate::guest::artifacts::guest_paths(
        crate::provider::target::Target::Linux,
        "sunlit-earth",
        "e2e-1a2b",
        false,
    );
    // The release build's own two jobs. The build one is the longest generated
    // script in the crate and the only one that runs for tens of minutes, so a
    // syntax error in it costs a boot and a source copy before it says anything.
    let pinned = crate::guest::toolchain::parse("[toolchain]\nchannel = \"1.94.0\"\n")
        .expect("a fixture pin parses");
    vec![
        (
            "dist: build, warm".to_owned(),
            crate::commands::dist::build_job(
                crate::provider::target::Target::Linux,
                &pinned,
                &crate::commands::dist::CacheJob {
                    restore: crate::store::cache::Kind::ALL.to_vec(),
                    save: crate::store::cache::Kind::ALL.to_vec(),
                },
            ),
        ),
        (
            "dist: build, cold".to_owned(),
            crate::commands::dist::build_job(
                crate::provider::target::Target::Linux,
                &pinned,
                &crate::commands::dist::CacheJob::default(),
            ),
        ),
        (
            "dist: verify the bundle".to_owned(),
            crate::commands::dist::verify_job(
                crate::provider::target::Target::Linux,
                &crate::commands::dist::Verification::Bundle {
                    exe: "/var/lib/sunlit-e2e/sunlit-earth-0.1.0-linux/sunlit-earth",
                    empty: "/var/lib/sunlit-e2e/empty-textures",
                },
            ),
        ),
        (
            "dist: verify the loose binary".to_owned(),
            crate::commands::dist::verify_job(
                crate::provider::target::Target::Linux,
                &crate::commands::dist::Verification::Loose {
                    exe: "/var/lib/sunlit-e2e/bin/sunlit-earth",
                },
            ),
        ),
        (
            "handover: launcher".to_owned(),
            crate::guest::handover::linux_launcher_script(&paths),
        ),
        (
            "handover: launcher without textures".to_owned(),
            crate::guest::handover::linux_launcher_script(&bare),
        ),
        (
            "handover: desktop entries".to_owned(),
            crate::guest::handover::linux_install_script(&paths),
        ),
    ]
}

/// Every shell script the repository ships: the ones the Linux image build
/// runs, and the user-local icon install that goes out beside the desktop
/// entry. The second one runs on someone else's machine, which is the worst
/// place for a syntax error to surface.
fn linux_guest_scripts() -> Vec<std::path::PathBuf> {
    let repo = crate::store::repo_root();
    let mut scripts = template_scripts(crate::provider::target::Target::Linux, "sh");
    scripts.extend(scripts_in(&repo.join("assets").join("linux"), "sh"));
    scripts
}

#[cfg(windows)]
/// The checker, which reads its payload from stdin.
///
/// The scripts cannot travel on the command line: encoded, all of them
/// together run past the 32767-character limit `CreateProcess` enforces. As
/// stdin they are data rather than code, so the multi-line parsing problem that
/// rules out `-Command -` does not arise.
const CHECKER: &str = r#"
$payload = [Console]::In.ReadToEnd()
foreach ($line in ($payload -split "`r?`n")) {
  if (-not $line.Trim()) { continue }
  $parts = $line -split "`t", 2
  $name = $parts[0]
  $text = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($parts[1]))
  $errors = $null
  $tokens = $null
  [void][System.Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
  if ($errors.Count -gt 0) {
    Write-Output "FAIL $name :: $($errors[0].Message)"
  } else {
    Write-Output "OK $name"
  }
}
"#;

/// One `name<TAB>base64` line per script.
#[cfg(windows)]
fn payload(scripts: &[(String, String)]) -> String {
    scripts
        .iter()
        .map(|(name, script)| format!("{name}	{}", encode_command(script)))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

/// Parse-check every script in one invocation.
#[cfg(windows)]
fn check(scripts: &[(String, String)]) -> crate::runner::CommandOutput {
    RealRunner
        .capture(&powershell(CHECKER).stdin(payload(scripts)))
        .expect("powershell.exe runs")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No shipped guest script may carry a control character.
    ///
    /// Found the expensive way while preparing the amendment. `finalize.ps1`
    /// held a literal backspace, 0x08, where `\b` had been written into a path:
    /// the file said `\EFI\Microsoft\Boot` and then a byte that made a terminal
    /// print the next character over the last one, so it read as
    /// `\EFI\Microsoft\Bootootmgfw.efi` and named a file that cannot exist.
    ///
    /// A parser does not care: inside a double-quoted string a backspace is a
    /// character like any other, so the `PowerShell` parse check passes it and
    /// the fault only surfaces as a path that is not found, in a script that
    /// runs forty minutes into an image build. This is the check of the same
    /// kind as the failure.
    #[test]
    fn no_guest_script_carries_a_control_character() {
        let mut checked = 0;
        for script in windows_guest_scripts("ps1")
            .into_iter()
            .chain(windows_guest_scripts("cmd"))
            .chain(linux_guest_scripts())
        {
            let bytes = std::fs::read(&script).expect("readable");
            for (index, byte) in bytes.iter().enumerate() {
                assert!(
                    !byte.is_ascii_control() || matches!(byte, b'\t' | b'\r' | b'\n'),
                    "{} carries a {byte:#04x} control character at byte {index}, \
                     which is what an escape such as \\b in a path leaves behind",
                    script.display()
                );
            }
            checked += 1;
        }
        assert!(checked >= 7, "only {checked} guest scripts checked");
    }

    #[test]
    fn every_batch_file_the_windows_guest_runs_is_usable() {
        // There is no parser to borrow for a batch file, so this checks the
        // two things that actually go wrong with one. `cmd.exe` mishandles
        // LF-only parenthesized blocks, and both of these have one, so they
        // carry CRLF (enforced in .gitattributes) while everything else in the
        // repo is LF.
        let scripts = windows_guest_scripts("cmd");
        assert!(
            scripts.len() >= 2,
            "only {} batch files found",
            scripts.len()
        );
        for script in scripts {
            let bytes = std::fs::read(&script).expect("readable");
            let text = String::from_utf8(bytes).expect("a batch file is text");
            assert!(!text.trim().is_empty(), "{} is empty", script.display());
            let lone_lf = text
                .char_indices()
                .filter(|&(i, c)| c == '\n' && (i == 0 || !text[..i].ends_with('\r')))
                .count();
            assert_eq!(
                lone_lf,
                0,
                "{} has {lone_lf} LF-only line endings; cmd.exe needs CRLF",
                script.display()
            );
        }
    }

    #[test]
    fn every_shell_script_the_linux_build_runs_parses() {
        // `bash -n` reads and parses without executing. Every platform this
        // suite runs on has a bash: the Linux and macOS runners natively, the
        // Windows ones through Git or WSL.
        //
        // The script arrives on stdin rather than as a file name, and that is
        // not a stylistic choice. On a Windows host `bash` on PATH is as likely
        // to be `System32\bash.exe`, the WSL launcher, as Git's, and the WSL one
        // cannot open a path from this side at all: `C:\...` reaches it with the
        // separators eaten as escapes, and `C:/...` names a directory that does
        // not exist inside the distribution. Either way the parse that was
        // supposed to be the test never happened, and it failed as a missing
        // file rather than as a syntax error. Handing over the text works on
        // every bash, and the file name appears in the assertion below rather
        // than in bash's message, which is where it was useful anyway.
        let scripts = linux_guest_scripts();
        assert!(scripts.len() >= 3, "only {} scripts found", scripts.len());

        let Some(bash) = RealRunner.which("bash") else {
            println!("skipping: no bash on PATH to parse the guest scripts with");
            return;
        };
        let parse = |text: String| {
            RealRunner
                .capture(
                    &crate::runner::Cmd::new(bash.to_string_lossy())
                        .arg("-n")
                        .stdin(text),
                )
                .expect("bash runs")
        };

        // A negative control, because everything below now rests on bash
        // reading its stdin: a pipe that delivered nothing would make every
        // script in the repository parse perfectly.
        assert!(
            !parse("echo 'unterminated\n".to_owned()).success(),
            "bash accepted an unterminated quote, so it is not reading what it is given"
        );
        // And a control for the failure bash only warns about: a heredoc whose
        // terminator never matches parses with exit 0 and a warning on stderr,
        // so an exit-code check alone would wave through a script the heredoc
        // has swallowed whole. The warning check below is what catches it.
        let swallowed = parse("cat <<'EOF'\nhello\nEOFX\n".to_owned());
        assert!(
            swallowed.success() && swallowed.stderr.contains("warning:"),
            "bash -n changed how it reports an unterminated heredoc \
             (exit {:?}, stderr {:?}); the warning check below rests on this",
            swallowed.code,
            swallowed.stderr.trim()
        );

        let mut texts: Vec<(String, String)> = scripts
            .into_iter()
            .map(|script| {
                let text = std::fs::read_to_string(&script)
                    .unwrap_or_else(|e| panic!("cannot read {}: {e}", script.display()));
                (script.display().to_string(), text)
            })
            .collect();
        // The shell the orchestrator sends over SSH belongs here too: it is
        // built by string formatting, which is the mistake this parser is for,
        // and nothing else reads it before a guest does.
        texts.push((
            "vm: place the screens".to_owned(),
            crate::commands::vm::place_screens_command(),
        ));
        texts.push((
            "vm: map the pointer".to_owned(),
            crate::commands::vm::map_pointer_command(),
        ));

        for (name, text) in texts {
            let out = parse(text);
            assert!(
                out.success() && !out.stderr.contains("warning:"),
                "{name} does not parse cleanly: {}",
                out.stderr.trim()
            );
        }

        // And the generated ones, which get the check for the same reason: the
        // hand-over's install script is three heredocs built by formatting, and
        // a syntax error in it reaches nobody but the guest.
        let generated = generated_linux_scripts();
        assert!(generated.len() >= 3, "only {} generated", generated.len());
        for (name, text) in generated {
            let out = parse(text);
            assert!(
                out.success() && !out.stderr.contains("warning:"),
                "{name} does not parse cleanly: {}",
                out.stderr.trim()
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn every_generated_powershell_script_parses() {
        let scripts = all_scripts();
        assert!(
            scripts.len() >= 10,
            "only {} scripts checked",
            scripts.len()
        );

        let out = check(&scripts);
        assert!(
            out.success(),
            "the checker itself failed: {:?} {}",
            out.code,
            out.stderr
        );

        let failures: Vec<&str> = out
            .stdout
            .lines()
            .filter(|line| line.starts_with("FAIL "))
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));

        let checked = out.stdout.lines().filter(|l| l.starts_with("OK ")).count();
        assert_eq!(
            checked,
            scripts.len(),
            "not every script was reported on:\n{}",
            out.stdout
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_checker_would_notice_a_broken_script() {
        // Without this, a checker that silently reported nothing would look
        // exactly like a clean run.
        let broken = vec![(
            "deliberately broken".to_owned(),
            "if ($x { Write-Output 'unclosed'".to_owned(),
        )];
        let out = check(&broken);
        assert!(
            out.stdout.contains("FAIL deliberately broken"),
            "{}",
            out.stdout
        );
    }
}
