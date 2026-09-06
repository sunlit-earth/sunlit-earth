//! Engine integration tests.
//!
//! These run the real engine against the real GPU pipeline, headlessly. They
//! assert behavioral invariants (a frame arrives, an unchanged scene does not
//! re-render, a changed one does) rather than pixel values, so they survive
//! adapter differences.
//!
//! An engine start costs about 1.3 seconds, almost all of it the wgpu device
//! and the seven pipelines compiled from `sphere.wgsl`, and none of it depends
//! on anything a case varies. So the cases are grouped by the configuration
//! they need, each group shares one engine for the whole run, and everything a
//! case does differ in reaches that engine as a command. What cannot be a
//! command is what a group is: the texture files behind the slots, the cloud
//! source, the sink and the clock are all read once at startup, and a case that
//! needs its own reads a `harness_of_its_own`.
//!
//! Every case takes `gpu()` first and holds it to the end, so exactly one
//! engine is rendering at any moment. The shared engines stay alive between
//! cases, which is the point of them; `Harness::reset` is what puts one back
//! the way its group expects to find it.

#[path = "../support/mod.rs"]
mod support;
#[path = "../../src/test_support.rs"]
mod test_support;

mod clouds;
mod clouds_variant;
mod display_change;
mod display_plan;
mod groups;
mod harness;
mod lifecycle;
mod memory;
mod moon;
mod panorama;
mod sinks;
mod stars;
mod sun;
mod textures;
