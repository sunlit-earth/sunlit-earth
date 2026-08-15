use tracing::{info, warn};

/// Result of initializing wgpu manually.
///
/// The instance and the adapter are not returned. They used to be, so the Slint
/// shell could assemble a `WGPUConfiguration::Manual` and share this device;
/// nothing does that any more, and the engine owns the device outright. wgpu's
/// own handles keep whatever they need alive, so dropping the instance and the
/// adapter at the end of `init` is fine.
pub struct WgpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: String,
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
pub fn init(force_software: bool) -> WgpuContext {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    assert!(!adapters.is_empty(), "No wgpu adapters found");

    let adapter = select_adapter(&adapters, force_software);
    let info = adapter.get_info();
    let adapter_info = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);
    info!(adapter = %adapter_info, "selected GPU adapter");

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

    WgpuContext {
        device,
        queue,
        adapter_info,
        supported_sample_counts,
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
        warn!("no software adapter found, using default selection");
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
        warn!(name = %name, "WGPU_ADAPTER_NAME not found, using default selection");
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
}
