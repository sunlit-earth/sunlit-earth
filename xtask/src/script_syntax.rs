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
    use crate::facts::{FEATURE_HYPERV, WINDOWS_QEMU_DIRS};
    use crate::provider::hyperv;
    use crate::setup::{SetupInputs, windows_plan};
    use crate::store::Store;

    let mut scripts = Vec::new();

    let probe = crate::facts::windows_probe_script(std::path::Path::new(r"C:\vm store"));
    scripts.push(("host probe".to_owned(), probe));

    let store = Store::new(r"C:\vm store");
    for step in windows_plan(&SetupInputs::default(), &store) {
        scripts.push((format!("setup: {}", step.name), step.script));
    }

    let name = "sunlit-e2e-windows";
    scripts.push((
        "hyperv: create".to_owned(),
        hyperv::create_script(name, r"C:\vm\golden.vhdx", r"C:\vm\overlay.vhdx"),
    ));
    scripts.push(("hyperv: state".to_owned(), hyperv::state_script(name)));
    scripts.push(("hyperv: address".to_owned(), hyperv::address_script(name)));
    scripts.push(("hyperv: destroy".to_owned(), hyperv::destroy_script(name)));

    // A couple of one-liners the providers build inline.
    scripts.push((
        "hyperv: start".to_owned(),
        format!("Start-VM -Name {}", crate::runner::ps_quote(name)),
    ));

    // The scripts that ship in the repo and run inside the guest. A typo in
    // one of these surfaces forty minutes into a Windows image build, which is
    // the most expensive place in this phase to find one.
    for name in ["bootstrap.ps1", "finalize.ps1"] {
        let path = crate::store::repo_root()
            .join("vm")
            .join("windows")
            .join("scripts")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        scripts.push((format!("guest: {name}"), text));
    }

    // Referenced so a rename here fails loudly rather than silently narrowing
    // what is checked.
    assert!(!FEATURE_HYPERV.is_empty() && !WINDOWS_QEMU_DIRS[0].is_empty());
    scripts
}

/// The shell scripts the Linux image build runs.
fn linux_guest_scripts() -> Vec<std::path::PathBuf> {
    let dir = crate::store::repo_root()
        .join("vm")
        .join("linux")
        .join("scripts");
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "sh"))
        .collect();
    out.sort();
    out
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

    #[test]
    fn every_shell_script_the_linux_build_runs_parses() {
        // `bash -n` reads and parses without executing. Every platform this
        // suite runs on has a bash: the Linux and macOS runners natively, the
        // Windows ones through Git.
        let scripts = linux_guest_scripts();
        assert!(scripts.len() >= 3, "only {} scripts found", scripts.len());

        let Some(bash) = RealRunner.which("bash") else {
            println!("skipping: no bash on PATH to parse the guest scripts with");
            return;
        };

        for script in scripts {
            let out = RealRunner
                .capture(
                    &crate::runner::Cmd::new(bash.to_string_lossy())
                        .args(["-n".to_owned(), script.to_string_lossy().into_owned()]),
                )
                .expect("bash runs");
            assert!(
                out.success(),
                "{} does not parse: {}",
                script.display(),
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
