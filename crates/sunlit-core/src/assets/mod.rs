//! Asset sources: texture files, cloud imagery, and the mailbox that carries
//! decoded frames from background threads to the consumer.

pub mod cloud_fetcher;
pub mod cloud_source;
pub mod mailbox;
pub mod stars;
pub mod texture_cache;
pub mod texture_loader;
