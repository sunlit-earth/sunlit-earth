use slint::wgpu_28::WGPUConfiguration;

/// Result of initializing wgpu manually.
pub struct WgpuContext {
    pub config: WGPUConfiguration,
    pub adapter_info: String,
}

/// Initialize wgpu manually: create instance, select adapter, request device.
/// This gives us full control over adapter selection and accurate adapter info.
///
/// Adapter selection priority:
/// 1. `WGPU_ADAPTER_NAME` env var (substring match, case-insensitive)
/// 2. Best available GPU (`DiscreteGpu` > `IntegratedGpu` > others)
/// 3. CPU/software fallback as last resort
pub fn init() -> WgpuContext {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    assert!(!adapters.is_empty(), "No wgpu adapters found");

    let adapter = select_adapter(&adapters);
    let info = adapter.get_info();
    let adapter_info = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sunlit-earth"),
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .expect("Failed to create wgpu device");

    WgpuContext {
        config: WGPUConfiguration::Manual {
            instance,
            adapter: adapter.clone(),
            device,
            queue,
        },
        adapter_info,
    }
}

fn select_adapter(adapters: &[wgpu::Adapter]) -> &wgpu::Adapter {
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
