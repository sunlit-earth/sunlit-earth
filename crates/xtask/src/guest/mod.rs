//! Work into and out of a guest: building the artifacts on the host, the job
//! scripts that run them inside, and the ssh/scp plumbing between the two.

pub mod artifacts;
pub mod cargo_json;
pub mod handover;
pub mod job;
#[cfg(test)]
pub mod script_syntax;
pub mod ssh;
pub mod toolchain;
