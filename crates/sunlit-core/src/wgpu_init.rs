use std::sync::OnceLock;

use tracing::{info, warn};

/// The process's one and only wgpu instance.
///
/// A `wgpu::Instance` is not a per-engine resource, it is a per-process one: it
/// owns the dynamically loaded driver libraries, and on Linux dropping the last
/// one `dlclose`s the Vulkan loader. Mesa registers pthread TLS destructors
/// that outlive that unload, so the next thread to exit calls a destructor
/// pointer into an unmapped page and the process dies with SIGSEGV inside
/// `__nptl_deallocate_tsd`. On lavapipe that killed the engine integration
/// tests at the moment the engine thread was joined: every time under
/// `cargo test`, and intermittently when a single test was run on its own,
/// which is the signature of a destructor list that only sometimes has an
/// entry to walk.
///
/// Keeping the instance in a `static` fixes it by construction: the loader is
/// never unloaded, because the instance is never dropped. It also stops the
/// suite from opening one Vulkan instance per engine, which was wasteful
/// regardless of the crash. Windows never showed this because unloading the
/// D3D12 runtime is safe; the defect was always in the code, only the platform
/// was forgiving.
static INSTANCE: OnceLock<wgpu::Instance> = OnceLock::new();

/// The process's wgpu instance, created on first use.
///
/// Public so the integration-test harnesses go through the same one; every
/// place that would otherwise write `wgpu::Instance::new` should call this
/// instead, so the invariant in `INSTANCE` holds for the whole process rather
/// than only for the engine.
pub fn instance() -> &'static wgpu::Instance {
    INSTANCE.get_or_init(|| {
        // The one call site. `clippy.toml` disallows the method everywhere so
        // that a second one has to be written on purpose.
        #[allow(clippy::disallowed_methods)]
        wgpu::Instance::new(&wgpu::InstanceDescriptor::default())
    })
}

/// Result of initializing wgpu manually.
///
/// The adapter is not returned. It used to be, so the Slint shell could
/// assemble a `WGPUConfiguration::Manual` and share this device; nothing does
/// that any more, and the engine owns the device outright. The device keeps
/// alive whatever it needs from the adapter, and the instance behind both is
/// the process-wide `INSTANCE` above.
pub struct WgpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: String,
    /// Short slug identifying which implementation rendered a frame.
    ///
    /// Golden references are per adapter, so they need a stable directory name;
    /// see `adapter_key`.
    pub adapter_key: String,
    /// Supported MSAA sample counts for the color format (always includes 1).
    ///
    /// Every requested count is resolved against this list before it can reach
    /// a render target; see `renderer::resolve_sample_count`.
    pub supported_sample_counts: Vec<u32>,
}

/// Initialize wgpu manually: create instance, select adapter, request device.
/// This gives us full control over adapter selection and accurate adapter info.
///
/// Adapter selection priority (when `force_software` is false):
/// 1. `WGPU_ADAPTER_NAME` env var (substring match, case-insensitive)
/// 2. Best available GPU (`DiscreteGpu` > `IntegratedGpu` > others)
/// 3. CPU/software fallback as last resort
///
/// A machine with no adapter at all, or one whose driver refuses a device, is
/// an ordinary thing to run into rather than a bug in this program, so both
/// come back as an error for the caller to report.
pub fn init(force_software: bool) -> Result<WgpuContext, String> {
    let adapters = pollster::block_on(instance().enumerate_adapters(wgpu::Backends::all()));

    let adapter = select_adapter(&adapters, force_software)
        .ok_or("no graphics adapter is available on this system")?;
    let info = adapter.get_info();
    let adapter_info = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);
    let adapter_key = adapter_key(&info.name, info.backend);
    info!(adapter = %adapter_info, key = %adapter_key, "selected GPU adapter");

    // Try to request adapter-specific format features for broader MSAA support.
    // Fall back to no extra features if unsupported.
    let desired_features = wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    let features = if adapter.features().contains(desired_features) {
        desired_features
    } else {
        wgpu::Features::empty()
    };

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sunlit-earth"),
        required_features: features,
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|e| format!("the graphics adapter \"{adapter_info}\" refused a device: {e}"))?;

    let supported_sample_counts = adapter
        .get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm)
        .flags
        .supported_sample_counts();

    Ok(WgpuContext {
        device,
        queue,
        adapter_info,
        adapter_key,
        supported_sample_counts,
    })
}

/// Short, filesystem-safe slug naming the implementation behind an adapter.
///
/// Golden references live one directory per adapter, and this names the
/// directory. The reason is margin, not raw incompatibility. Measured on the
/// four reference scenes, each against WARP, with a tolerance of mean 2.0 and
/// 1% outliers: lavapipe agrees to a mean channel difference of 0.19 to 0.86
/// with 0.001% to 0.42% outliers, and the paravirtual Metal device on a
/// `macos-latest` runner to 0.008 to 0.18 with at most 0.008% outliers. One
/// shared set would therefore pass on all three today, but it would spend up to
/// 43% of the mean budget on the difference between two correct
/// implementations, leaving a regression that large able to hide on one
/// platform while failing on another. Per-adapter references give each platform
/// the whole tolerance to spend on detecting real change.
///
/// The ordering in those numbers was not the expected one: the two CPU
/// rasterizers are the pair that disagree most, and Metal, which is both a
/// different shader translation target and an actual GPU, lands about five
/// times closer to WARP than lavapipe does.
///
/// Software rasterizers are keyed by name rather than by backend, because the
/// backend is the wrong granularity for them: two CPU implementations can sit
/// behind the same backend (lavapipe and `SwiftShader` are both Vulkan) and there
/// is no reason to expect their pixels to match. Hardware adapters are keyed by
/// backend, which is the right granularity there: what moves the pixels is the
/// shader translation target and the driver, and one directory per vendor and
/// model would produce a reference set nobody could regenerate.
///
/// The Mesa match is deliberately backend-blind and that is a known coarseness:
/// "llvmpipe" is the name reported by both Mesa's Vulkan software driver
/// (lavapipe) and its GL one, so the two would share the `lavapipe` directory
/// despite translating shaders to different targets. Nothing generates
/// references through the GL path today, because the tests force the software
/// adapter and CI installs `mesa-vulkan-drivers`; if something ever does, the
/// GL case needs the backend appended to its key.
pub fn adapter_key(name: &str, backend: wgpu::Backend) -> String {
    let lower = name.to_lowercase();

    // Mesa's Vulkan software driver is called lavapipe; it reports itself with
    // the name of the LLVM rasterizer underneath it, "llvmpipe".
    if lower.contains("lavapipe") || lower.contains("llvmpipe") {
        return "lavapipe".to_owned();
    }
    // Direct3D's software rasterizer, which reports as "Microsoft Basic Render
    // Driver" rather than by its WARP codename.
    if lower.contains("warp") || lower.contains("basic render driver") {
        return "warp".to_owned();
    }
    if lower.contains("swiftshader") {
        return "swiftshader".to_owned();
    }

    // `to_str` rather than the `Debug` output: wgpu documents these strings
    // ("vulkan", "metal", "dx12", "gl"), while `Debug` carries no stability
    // guarantee at all, and a directory name is a thing this repository commits.
    backend.to_str().to_owned()
}

/// Rank a GPU device type for adapter selection priority.
/// Lower is better: discrete GPU is preferred, CPU is last resort.
pub(crate) fn adapter_type_rank(device_type: wgpu::DeviceType) -> u32 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

fn select_adapter(adapters: &[wgpu::Adapter], force_software: bool) -> Option<&wgpu::Adapter> {
    if force_software {
        if let Some(adapter) = adapters
            .iter()
            .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
        {
            return Some(adapter);
        }
        warn!("no software adapter found, using default selection");
    }

    // Respect WGPU_ADAPTER_NAME env var
    if let Ok(name) = std::env::var("WGPU_ADAPTER_NAME") {
        let name_lower = name.to_lowercase();
        if let Some(adapter) = adapters
            .iter()
            .find(|a| a.get_info().name.to_lowercase().contains(&name_lower))
        {
            return Some(adapter);
        }
        warn!(name = %name, "WGPU_ADAPTER_NAME not found, using default selection");
    }

    adapters
        .iter()
        .min_by_key(|a| adapter_type_rank(a.get_info().device_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_rasterizers_are_keyed_by_name() {
        // The names are the ones the drivers actually report.
        assert_eq!(
            adapter_key("llvmpipe (LLVM 15.0.7, 256 bits)", wgpu::Backend::Vulkan),
            "lavapipe"
        );
        assert_eq!(
            adapter_key("Microsoft Basic Render Driver", wgpu::Backend::Dx12),
            "warp"
        );
        assert_eq!(
            adapter_key("SwiftShader Device (LLVM 16.0.0)", wgpu::Backend::Vulkan),
            "swiftshader"
        );
    }

    /// Two software rasterizers on one backend must not share a key, or one of
    /// them would be compared against the other's references.
    #[test]
    fn two_software_rasterizers_on_one_backend_differ() {
        assert_ne!(
            adapter_key("llvmpipe (LLVM 15.0.7, 256 bits)", wgpu::Backend::Vulkan),
            adapter_key("SwiftShader Device", wgpu::Backend::Vulkan)
        );
    }

    #[test]
    fn hardware_adapters_are_keyed_by_backend() {
        assert_eq!(adapter_key("Apple M2 Pro", wgpu::Backend::Metal), "metal");
        assert_eq!(
            adapter_key("Apple Paravirtual device", wgpu::Backend::Metal),
            "metal"
        );
        assert_eq!(
            adapter_key("NVIDIA GeForce RTX 4080", wgpu::Backend::Vulkan),
            "vulkan"
        );
        // The ordinary Windows case, and the one with a digit in its key.
        assert_eq!(
            adapter_key("NVIDIA GeForce RTX 4080", wgpu::Backend::Dx12),
            "dx12"
        );
    }

    /// The key names a directory, so it has to survive being one.
    #[test]
    fn keys_are_filesystem_safe() {
        for (name, backend) in [
            ("llvmpipe (LLVM 15.0.7, 256 bits)", wgpu::Backend::Vulkan),
            ("Microsoft Basic Render Driver", wgpu::Backend::Dx12),
            ("Apple Paravirtual device", wgpu::Backend::Metal),
            ("Intel(R) Arc(tm) A770", wgpu::Backend::Vulkan),
            // Hardware on D3D12, which is what most Windows machines report and
            // the reason digits are allowed below: its key is "dx12".
            ("AMD Radeon RX 7900 XTX", wgpu::Backend::Dx12),
            ("Mesa Intel(R) UHD Graphics", wgpu::Backend::Gl),
        ] {
            let key = adapter_key(name, backend);
            assert!(!key.is_empty(), "{name} produced an empty key");
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{name} produced {key}, which is not a plain directory name"
            );
        }
    }

    #[test]
    fn discrete_gpu_ranks_best() {
        assert!(
            adapter_type_rank(wgpu::DeviceType::DiscreteGpu)
                < adapter_type_rank(wgpu::DeviceType::IntegratedGpu)
        );
        assert!(
            adapter_type_rank(wgpu::DeviceType::DiscreteGpu)
                < adapter_type_rank(wgpu::DeviceType::VirtualGpu)
        );
        assert!(
            adapter_type_rank(wgpu::DeviceType::DiscreteGpu)
                < adapter_type_rank(wgpu::DeviceType::Other)
        );
        assert!(
            adapter_type_rank(wgpu::DeviceType::DiscreteGpu)
                < adapter_type_rank(wgpu::DeviceType::Cpu)
        );
    }

    #[test]
    fn cpu_ranks_worst() {
        assert!(
            adapter_type_rank(wgpu::DeviceType::Cpu)
                > adapter_type_rank(wgpu::DeviceType::DiscreteGpu)
        );
        assert!(
            adapter_type_rank(wgpu::DeviceType::Cpu)
                > adapter_type_rank(wgpu::DeviceType::IntegratedGpu)
        );
        assert!(
            adapter_type_rank(wgpu::DeviceType::Cpu)
                > adapter_type_rank(wgpu::DeviceType::VirtualGpu)
        );
        assert!(
            adapter_type_rank(wgpu::DeviceType::Cpu) > adapter_type_rank(wgpu::DeviceType::Other)
        );
    }

    #[test]
    fn full_ordering() {
        let ranks: Vec<u32> = [
            wgpu::DeviceType::DiscreteGpu,
            wgpu::DeviceType::IntegratedGpu,
            wgpu::DeviceType::VirtualGpu,
            wgpu::DeviceType::Other,
            wgpu::DeviceType::Cpu,
        ]
        .iter()
        .map(|&dt| adapter_type_rank(dt))
        .collect();
        // Each rank should be strictly less than the next
        for w in ranks.windows(2) {
            assert!(w[0] < w[1], "Expected {}<{}", w[0], w[1]);
        }
    }
}
