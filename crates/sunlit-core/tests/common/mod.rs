//! Shared GPU test infrastructure for integration tests.
//!
//! Provides a reusable `GpuContext` with device/queue creation (with software
//! fallback) and buffer readback helpers. Uses `LazyLock<Mutex<GpuContext>>`
//! to share a single device across parallel test threads, avoiding the
//! Windows crash from per-test device creation.

use std::sync::{LazyLock, Mutex, mpsc};

/// Shared GPU device and queue for integration tests.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Create a GPU context, optionally forcing the software adapter.
pub fn create_gpu_context(force_software: bool) -> GpuContext {
    pollster::block_on(async {
        let instance = wgpu::Instance::default();

        let adapter: wgpu::Adapter = if force_software {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: None,
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
                .expect("no software adapter available")
        } else {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    ..Default::default()
                })
                .await
                .or_else(|_| {
                    pollster::block_on(instance.request_adapter(
                        &wgpu::RequestAdapterOptions {
                            compatible_surface: None,
                            force_fallback_adapter: true,
                            ..Default::default()
                        },
                    ))
                })
                .expect("no wgpu adapter available (tried hardware and software)")
        };

        let (device, queue): (wgpu::Device, wgpu::Queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("failed to create wgpu device");

        GpuContext { device, queue }
    })
}

/// Global shared GPU context for tests (hardware preferred, software fallback).
#[allow(dead_code)]
pub static GPU: LazyLock<Mutex<GpuContext>> = LazyLock::new(|| {
    Mutex::new(create_gpu_context(false))
});

#[allow(dead_code)]
/// Read back a GPU buffer's contents as a `Vec<u8>`.
pub fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    queue.submit(std::iter::once(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().expect("buffer mapping failed");

    let mapped = slice.get_mapped_range();
    let data = mapped.to_vec();
    drop(mapped);
    staging.unmap();

    data
}

#[allow(unused_imports)]
pub use sunlit_core::renderer::read_texture_rgba8;
