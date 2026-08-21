//! Latest-value mailbox carrying decoded textures from background threads to
//! whoever uploads them to the GPU.

use std::sync::{Arc, Mutex};

use tracing::warn;

use super::texture_loader;

/// Message sent from a background decode thread when texture loading completes.
pub struct DecodedTextureMessage {
    pub slot_index: usize,
    pub result: Result<texture_loader::DecodedImage, String>,
    /// The texture generation the decode was spawned in, for the slots the
    /// resolution setting governs. A switch bumps the generation, so a decode of
    /// the old width that is still running when the user changes it arrives
    /// stamped with a generation the consumer no longer wants.
    ///
    /// `None` for a producer the resolution does not govern (the cloud
    /// fetcher), whose posts are never stale.
    pub generation: Option<u64>,
}

/// Latest-value mailbox carrying decoded textures from background threads to
/// the renderer, one slot per texture.
///
/// This replaces the unbounded channel that used to hold decoded pixel buffers:
/// only the most recent frame per slot is ever useful, so `post` overwrites the
/// parked message instead of queueing behind it. Parked memory is therefore
/// capped at one decoded frame per slot no matter how long the consumer stalls,
/// which is exactly what hiding the window to the tray used to do to it.
#[derive(Clone)]
pub struct TextureMailbox {
    slots: Arc<Mutex<Vec<Option<DecodedTextureMessage>>>>,
}

impl TextureMailbox {
    /// Create a mailbox with `slot_count` empty slots.
    pub fn new(slot_count: usize) -> Self {
        Self {
            slots: Arc::new(Mutex::new(
                std::iter::repeat_with(|| None).take(slot_count).collect(),
            )),
        }
    }

    /// Park a message in its slot, replacing anything not yet consumed.
    ///
    /// The slot count is fixed at construction, so an out-of-range index is a
    /// producer bug. The message is dropped with a `warn!` rather than growing
    /// the mailbox, which would turn that bug into an index-out-of-bounds panic
    /// in the consumer when it indexes its own slot array.
    pub fn post(&self, msg: DecodedTextureMessage) {
        let index = msg.slot_index;
        let mut slots = self.slots.lock().expect("texture mailbox lock poisoned");
        if index >= slots.len() {
            warn!(
                slot = index,
                slot_count = slots.len(),
                "texture mailbox slot out of range, dropping message"
            );
            return;
        }
        slots[index] = Some(msg);
    }

    /// Take every parked message, leaving the mailbox empty.
    pub fn take_all(&self) -> Vec<DecodedTextureMessage> {
        let mut slots = self.slots.lock().expect("texture mailbox lock poisoned");
        slots.iter_mut().filter_map(Option::take).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message carrying a 1x1 image tagged with `width` so tests can tell
    /// successive posts to the same slot apart.
    fn message(slot_index: usize, width: u32) -> DecodedTextureMessage {
        DecodedTextureMessage {
            slot_index,
            result: Ok(texture_loader::DecodedImage {
                pixels: vec![0; 4],
                width,
                height: 1,
            }),
            generation: None,
        }
    }

    /// Extract the tag written by `message`.
    fn tag(msg: &DecodedTextureMessage) -> u32 {
        msg.result.as_ref().expect("test messages are Ok").width
    }

    #[test]
    fn mailbox_take_all_empties_the_mailbox() {
        let mailbox = TextureMailbox::new(4);
        mailbox.post(message(0, 1));
        mailbox.post(message(3, 2));

        let mut taken = mailbox.take_all();
        taken.sort_by_key(|m| m.slot_index);
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0].slot_index, 0);
        assert_eq!(taken[1].slot_index, 3);

        assert!(
            mailbox.take_all().is_empty(),
            "second take should find nothing"
        );
    }

    #[test]
    fn mailbox_replaces_unconsumed_message_in_same_slot() {
        let mailbox = TextureMailbox::new(4);
        for tag_value in 1..=5 {
            mailbox.post(message(2, tag_value));
        }

        let taken = mailbox.take_all();
        assert_eq!(taken.len(), 1, "only the newest frame per slot is kept");
        assert_eq!(tag(&taken[0]), 5, "the newest frame must win");
    }

    #[test]
    fn mailbox_keeps_one_message_per_slot_independently() {
        let mailbox = TextureMailbox::new(4);
        mailbox.post(message(0, 1));
        mailbox.post(message(1, 1));
        mailbox.post(message(0, 2));

        let mut taken = mailbox.take_all();
        taken.sort_by_key(|m| m.slot_index);
        assert_eq!(taken.len(), 2);
        assert_eq!(tag(&taken[0]), 2, "slot 0 keeps the newer frame");
        assert_eq!(tag(&taken[1]), 1, "slot 1 is untouched");
    }

    #[test]
    fn mailbox_drops_out_of_range_slot_instead_of_growing() {
        let mailbox = TextureMailbox::new(2);
        mailbox.post(message(7, 1));

        assert!(
            mailbox.take_all().is_empty(),
            "an out-of-range post must be dropped, not parked"
        );
        // The mailbox must still accept valid slots afterwards.
        mailbox.post(message(1, 1));
        assert_eq!(mailbox.take_all().len(), 1);
    }
}
