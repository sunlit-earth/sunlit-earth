#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod cloud_fetcher;
pub mod config;
pub mod geometry;
pub mod headless;
pub mod renderer;
pub mod scene;
pub mod texture_loader;
#[cfg(windows)]
pub mod wallpaper;
pub mod wgpu_init;

slint::include_modules!();
