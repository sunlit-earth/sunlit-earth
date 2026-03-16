#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod geometry;
pub mod renderer;
pub mod scene;
pub mod texture_loader;
#[cfg(windows)]
pub mod wallpaper;
pub mod wgpu_init;

slint::include_modules!();
