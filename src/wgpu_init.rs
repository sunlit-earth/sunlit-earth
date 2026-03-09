use slint::wgpu_28::WGPUConfiguration;

/// Result of initializing wgpu manually.
pub struct WgpuContext {
    pub config: WGPUConfiguration,
    pub adapter_info: String,
    /// Supported MSAA sample counts for the color format (always includes 1).
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
        .get_texture_format_features(wgpu::TextureFormat::Rgba8UnormSrgb)
        .flags
        .supported_sample_counts();

    WgpuContext {
        config: WGPUConfiguration::Manual {
            instance,
            adapter: adapter.clone(),
            device,
            queue,
        },
        adapter_info,
        supported_sample_counts,
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

    // Rank by device type preference
    let rank = |adapter: &wgpu::Adapter| match adapter.get_info().device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    };

    adapters
        .iter()
        .min_by_key(|a| rank(a))
        .expect("No adapters available")
}
