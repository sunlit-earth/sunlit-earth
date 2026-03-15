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

#[allow(dead_code)]
/// Read back a 2D texture as RGBA8 pixel data.
pub fn read_texture_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    // bytes_per_row must be aligned to 256 for buffer-texture copies
    let bytes_per_row_unaligned = width * 4;
    let bytes_per_row = (bytes_per_row_unaligned + 255) & !255;
    let buffer_size = u64::from(bytes_per_row) * u64::from(height);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture_readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().expect("buffer mapping failed");

    let mapped = slice.get_mapped_range();
    // If bytes_per_row != width*4, we need to strip padding
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let end = start + (width * 4) as usize;
        pixels.extend_from_slice(&mapped[start..end]);
    }
    drop(mapped);
    readback.unmap();

    pixels
}
