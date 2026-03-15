#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod camera;
pub mod grid_texture;
pub mod renderer;
pub mod sphere;
pub mod sun;
pub mod texture_loader;
pub mod wgpu_init;

slint::include_modules!();
