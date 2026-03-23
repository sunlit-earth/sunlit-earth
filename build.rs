fn main() {
    slint_build::compile("ui/main.slint").unwrap();

    // Increase the Windows main thread stack from 1 MB to 4 MB.
    // The Slint + wgpu rendering pipeline has deep call chains (backend init,
    // shader compilation, GPU resource creation) that can exceed 1 MB in debug
    // builds where LLVM keeps all locals on the stack without optimization.
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-arg=/STACK:4194304");
}
