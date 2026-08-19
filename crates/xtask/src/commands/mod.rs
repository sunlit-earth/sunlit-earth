//! The user-facing commands, one module per subcommand.
//!
//! Each stays thin: decisions live in the domain modules (`host`, `store`,
//! `provider`, `guest`) and everything effectful goes through
//! `runner::Runner`.

pub mod build_image;
pub mod destroy;
pub mod doctor;
pub mod e2e;
pub mod setup;
pub mod status;
pub mod vm;
