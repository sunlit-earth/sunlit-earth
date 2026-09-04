//! The user-facing commands, one module per subcommand.
//!
//! Each stays thin: decisions live in the domain modules (`host`, `store`,
//! `provider`, `guest`) and everything effectful goes through
//! `runner::Runner`.

pub mod bake_icon;
pub mod bake_stars;
pub mod build_hyperv;
pub mod build_image;
pub mod build_layer;
pub mod build_watch;
pub mod bundle;
pub mod dist;
pub mod doctor;
pub mod e2e;
pub mod setup;
pub mod status;
pub mod sweep;
pub mod teardown;
pub mod vm;
