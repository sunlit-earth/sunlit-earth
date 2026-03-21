use slint::wgpu_28::WGPUConfiguration;

/// Result of initializing wgpu manually (for Slint integration).
pub struct WgpuContext {
    pub config: WGPUConfiguration,
    pub adapter_info: String,
    /// Supported MSAA sample counts for the color format (always includes 1).
    pub supported_sample_counts: Vec<u32>,
}

/// Result of initializing wgpu without Slint (for headless rendering).
pub struct HeadlessContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: String,
    /// Supported MSAA sample counts for the color format (always includes 1).
    pub supported_sample_counts: Vec<u32>,
}

/// Shared result of adapter selection and device creation.
struct RawGpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: String,
    supported_sample_counts: Vec<u32>,
}

/// Core adapter selection and device creation, shared by both `init()` and
/// `init_headless()`.
fn init_raw(force_software: bool) -> RawGpuContext {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    assert!(!adapters.is_empty(), "No wgpu adapters found");

    let adapter = select_adapter(&adapters, force_software);
    let info = adapter.get_info();
    let adapter_info = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);

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
    .expect("Failed to create wgpu device");

    let supported_sample_counts = adapter
        .get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm)
        .flags
        .supported_sample_counts();

    RawGpuContext {
        instance,
        adapter: adapter.clone(),
        device,
        queue,
        adapter_info,
        supported_sample_counts,
    }
}

/// Initialize wgpu manually: create instance, select adapter, request device.
/// This gives us full control over adapter selection and accurate adapter info.
///
/// Adapter selection priority (when `force_software` is false):
/// 1. `WGPU_ADAPTER_NAME` env var (substring match, case-insensitive)
/// 2. Best available GPU (`DiscreteGpu` > `IntegratedGpu` > others)
/// 3. CPU/software fallback as last resort
pub fn init(force_software: bool) -> WgpuContext {
    let raw = init_raw(force_software);

    WgpuContext {
        config: WGPUConfiguration::Manual {
            instance: raw.instance,
            adapter: raw.adapter,
            device: raw.device,
            queue: raw.queue,
        },
        adapter_info: raw.adapter_info,
        supported_sample_counts: raw.supported_sample_counts,
    }
}

/// Initialize wgpu for headless rendering (no Slint dependency).
///
/// Returns a raw device/queue pair suitable for standalone rendering
/// without a Slint window. Uses the same adapter selection logic as
/// `init()`.
pub fn init_headless(force_software: bool) -> HeadlessContext {
    let raw = init_raw(force_software);

    HeadlessContext {
        device: raw.device,
        queue: raw.queue,
        adapter_info: raw.adapter_info,
        supported_sample_counts: raw.supported_sample_counts,
    }
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

fn select_adapter(adapters: &[wgpu::Adapter], force_software: bool) -> &wgpu::Adapter {
    if force_software {
        if let Some(adapter) = adapters
            .iter()
            .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
        {
            return adapter;
        }
        eprintln!("Warning: no software adapter found, using default selection");
    }

    // Respect WGPU_ADAPTER_NAME env var
    if let Ok(name) = std::env::var("WGPU_ADAPTER_NAME") {
        let name_lower = name.to_lowercase();
        if let Some(adapter) = adapters
            .iter()
            .find(|a| a.get_info().name.to_lowercase().contains(&name_lower))
        {
            return adapter;
        }
        eprintln!("Warning: WGPU_ADAPTER_NAME={name:?} not found, using default selection");
    }

    adapters
        .iter()
        .min_by_key(|a| adapter_type_rank(a.get_info().device_type))
        .expect("No adapters available")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discrete_gpu_ranks_best() {
        assert!(adapter_type_rank(wgpu::DeviceType::DiscreteGpu) < adapter_type_rank(wgpu::DeviceType::IntegratedGpu));
        assert!(adapter_type_rank(wgpu::DeviceType::DiscreteGpu) < adapter_type_rank(wgpu::DeviceType::VirtualGpu));
        assert!(adapter_type_rank(wgpu::DeviceType::DiscreteGpu) < adapter_type_rank(wgpu::DeviceType::Other));
        assert!(adapter_type_rank(wgpu::DeviceType::DiscreteGpu) < adapter_type_rank(wgpu::DeviceType::Cpu));
    }

    #[test]
    fn cpu_ranks_worst() {
        assert!(adapter_type_rank(wgpu::DeviceType::Cpu) > adapter_type_rank(wgpu::DeviceType::DiscreteGpu));
        assert!(adapter_type_rank(wgpu::DeviceType::Cpu) > adapter_type_rank(wgpu::DeviceType::IntegratedGpu));
        assert!(adapter_type_rank(wgpu::DeviceType::Cpu) > adapter_type_rank(wgpu::DeviceType::VirtualGpu));
        assert!(adapter_type_rank(wgpu::DeviceType::Cpu) > adapter_type_rank(wgpu::DeviceType::Other));
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

    #[test]
    fn init_headless_returns_usable_context() {
        let ctx = init_headless(false);
        assert!(
            ctx.supported_sample_counts.contains(&1),
            "sample counts should include 1"
        );
        // Verify device is usable by creating a simple buffer
        let _buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test_buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
    }

    #[test]
    fn init_headless_adapter_info_nonempty() {
        let ctx = init_headless(false);
        assert!(
            !ctx.adapter_info.is_empty(),
            "adapter info should be non-empty"
        );
    }
}
