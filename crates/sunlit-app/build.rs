use std::path::{Path, PathBuf};

fn main() {
    refuse_a_foreign_target_directory();

    let config = slint_build::CompilerConfiguration::new().with_debug_info(true);
    slint_build::compile_with_config("ui/main.slint", config).unwrap();

    // The exe icon. `embed-resource` compiles the resource script and tells
    // cargo to link the result; it no-ops for a non-Windows target, and the
    // dependency itself is only present on a Windows host.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=sunlit-earth.rc");
        println!("cargo:rerun-if-changed=../../assets/icon/baked/sunlit-earth.ico");
        embed_resource::compile("sunlit-earth.rc", embed_resource::NONE)
            .manifest_required()
            .expect("compiling the Windows icon resource");
    }

    // Increase the Windows main thread stack from 1 MB to 4 MB.
    // The Slint + wgpu rendering pipeline has deep call chains (backend init,
    // shader compilation, GPU resource creation) that can exceed 1 MB in debug
    // builds where LLVM keeps all locals on the stack without optimization.
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-arg=/STACK:4194304");
}

/// Fail the build when another platform has already built into this profile
/// directory.
///
/// The Linux port is developed through WSL, which sees the repository at
/// `/mnt/c/...` and, with `CARGO_TARGET_DIR` unset, builds into the same
/// `target/` the Windows build uses. Nothing collides when it does: the target
/// triple is part of every artifact hash, so cargo writes a second complete set
/// of artifacts beside the first, reports nothing unusual, and the directory
/// holds two platforms' worth of everything. README's WSL command sets the
/// variable; this is what happens when it is forgotten.
///
/// A build script cannot run before the dependencies it and its crate need are
/// compiled, so this catches the linked binaries rather than every rlib. That
/// is where the weight is: a debug test binary of this workspace is several
/// hundred megabytes and there is one per integration target.
fn refuse_a_foreign_target_directory() {
    let (Ok(out_dir), Ok(target)) = (std::env::var("OUT_DIR"), std::env::var("TARGET")) else {
        return;
    };
    let Some(profile) = profile_dir(Path::new(&out_dir)) else {
        return;
    };

    let marker = profile.join(".built-for");
    let previous = std::fs::read_to_string(&marker).unwrap_or_default();
    match previous.trim() {
        "" => {
            // Nobody has claimed it, or the claim did not survive; either way
            // this build is the one writing here. A failure to record that is
            // not a reason to fail the build.
            let _ = std::fs::write(&marker, format!("{target}\n"));
        }
        recorded if recorded != target => {
            eprintln!();
            eprintln!("this target directory belongs to another platform");
            eprintln!();
            eprintln!("  {}", profile.display());
            eprintln!("  holds artifacts for {recorded}, and this build is for {target}.");
            eprintln!();
            eprintln!("  The two would not collide. The target triple is part of every");
            eprintln!("  artifact hash, so cargo would write a second complete set beside");
            eprintln!("  the first and never mention it.");
            eprintln!();
            eprintln!("  Give this build its own directory, the way README's WSL command");
            eprintln!("  does:");
            eprintln!();
            eprintln!("    CARGO_TARGET_DIR=$HOME/sunlit-target cargo ...");
            eprintln!();
            eprintln!("  or hand this one over with `cargo clean`.");
            eprintln!();
            std::process::exit(1);
        }
        _ => {}
    }
}

/// The profile directory holding this build's artifacts.
///
/// `OUT_DIR` is `<target>/<profile>/build/<package>-<hash>/out`, with a triple
/// above `<profile>` when `--target` was given. Either way the profile
/// directory is four levels up, and it is the right granularity: a build for
/// an explicit triple lands under its own and shares nothing.
fn profile_dir(out_dir: &Path) -> Option<PathBuf> {
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}
