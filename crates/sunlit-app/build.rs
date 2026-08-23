fn main() {
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
