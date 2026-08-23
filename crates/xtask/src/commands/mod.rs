//! The user-facing commands, one module per subcommand.
//!
//! Each stays thin: decisions live in the domain modules (`host`, `store`,
//! `provider`, `guest`) and everything effectful goes through
//! `runner::Runner`.

pub mod bake_icon;
pub mod build_hyperv;
pub mod build_image;
pub mod build_watch;
pub mod doctor;
pub mod e2e;
pub mod setup;
pub mod status;
pub mod teardown;
pub mod vm;
