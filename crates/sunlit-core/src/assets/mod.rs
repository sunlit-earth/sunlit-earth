//! Asset sources: texture files, cloud imagery, and the mailbox that carries
//! decoded frames from background threads to the consumer.

pub mod cloud_fetcher;
pub mod mailbox;
pub mod texture_loader;
