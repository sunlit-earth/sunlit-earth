//! Latest-value mailbox carrying decoded textures from background threads to
//! whoever uploads them to the GPU.

use std::sync::{Arc, Mutex};

use tracing::{debug, warn};

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
/// Only the most recent frame per slot is ever useful, so `post` overwrites the
/// parked message instead of queueing behind it. Parked memory is therefore
/// capped at one decoded frame per slot no matter how long the consumer stalls.
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

    /// How many slots this mailbox has, which is fixed at construction.
    ///
    /// The consumer indexes its own array by the same slot numbers, so a
    /// mailbox that disagrees with it either drops messages for the high slots
    /// or hands over an index that array does not have. `Engine::new` compares
    /// the two rather than trusting them to match.
    pub(crate) fn slot_count(&self) -> usize {
        self.slots
            .lock()
            .expect("texture mailbox lock poisoned")
            .len()
    }

    /// Park a message in its slot, replacing anything not yet consumed, unless
    /// what is parked is from a newer generation than the arrival.
    ///
    /// The slot count is fixed at construction, so an out-of-range index is a
    /// producer bug. The message is dropped with a `warn!` rather than growing
    /// the mailbox, which would turn that bug into an index-out-of-bounds panic
    /// in the consumer when it indexes its own slot array.
    ///
    /// The generation guard is what makes "latest value" mean the newest thing
    /// anyone still wants rather than the last thing to arrive: a stale decode
    /// landing on a fresh one would leave no copy the consumer will take. Within
    /// one generation the newest arrival still wins.
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
        if let Some(parked) = &slots[index]
            && supersedes(parked.generation, msg.generation)
        {
            debug!(
                slot = index,
                parked = ?parked.generation,
                arriving = ?msg.generation,
                "keeping the newer parked texture, dropping a superseded arrival"
            );
            return;
        }
        slots[index] = Some(msg);
    }

    /// Take every parked message, leaving the mailbox empty.
    pub(crate) fn take_all(&self) -> Vec<DecodedTextureMessage> {
        let mut slots = self.slots.lock().expect("texture mailbox lock poisoned");
        slots.iter_mut().filter_map(Option::take).collect()
    }
}

/// Whether what is already parked is from a newer generation than what is
/// arriving, and must therefore be kept.
///
/// Only two stamped messages can be ordered. A producer that carries no
/// generation is one the ordering does not apply to (the cloud fetcher), and
/// nothing it posts is ever held back or held onto.
fn supersedes(parked: Option<u64>, arriving: Option<u64>) -> bool {
    matches!((parked, arriving), (Some(parked), Some(arriving)) if parked > arriving)
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

    /// The same, stamped with the generation it was spawned in.
    fn stamped(slot_index: usize, width: u32, generation: u64) -> DecodedTextureMessage {
        DecodedTextureMessage {
            generation: Some(generation),
            ..message(slot_index, width)
        }
    }

    /// Extract the generation of the single message parked in `slot`.
    fn parked_generation(mailbox: &TextureMailbox, slot: usize) -> Option<u64> {
        let taken = mailbox.take_all();
        assert_eq!(taken.len(), 1, "expected exactly one parked message");
        assert_eq!(taken[0].slot_index, slot);
        taken[0].generation
    }

    /// Extract the tag written by `message`.
    fn tag(msg: &DecodedTextureMessage) -> u32 {
        msg.result.as_ref().expect("test messages are Ok").width
    }

    /// Slots are independent of each other, and a take empties every one of
    /// them.
    #[test]
    fn mailbox_take_all_empties_every_slot() {
        let mailbox = TextureMailbox::new(4);
        mailbox.post(message(0, 1));
        mailbox.post(message(3, 2));
        mailbox.post(message(0, 3));

        let mut taken = mailbox.take_all();
        taken.sort_by_key(|m| m.slot_index);
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0].slot_index, 0);
        assert_eq!(tag(&taken[0]), 3, "slot 0 keeps the newer frame");
        assert_eq!(taken[1].slot_index, 3);
        assert_eq!(tag(&taken[1]), 2, "slot 3 is untouched by slot 0");

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

    /// A decode of a superseded resolution finishing after its own replacement
    /// is already parked.
    #[test]
    fn mailbox_keeps_the_newer_generation_whichever_order_the_two_arrive_in() {
        let mailbox = TextureMailbox::new(4);
        mailbox.post(stamped(1, 2048, 2));
        mailbox.post(stamped(1, 8192, 1));
        assert_eq!(
            parked_generation(&mailbox, 1),
            Some(2),
            "the stale arrival must not replace its own replacement"
        );

        mailbox.post(stamped(1, 8192, 1));
        mailbox.post(stamped(1, 2048, 2));
        assert_eq!(parked_generation(&mailbox, 1), Some(2));
    }

    /// Within one generation nothing is ordered and the newest arrival wins,
    /// which is the plain latest-value behavior the cloud fetcher depends on.
    #[test]
    fn mailbox_still_takes_the_newest_message_of_the_same_generation() {
        let mailbox = TextureMailbox::new(4);
        mailbox.post(stamped(1, 1, 7));
        mailbox.post(stamped(1, 2, 7));

        let taken = mailbox.take_all();
        assert_eq!(tag(&taken[0]), 2);
    }

    #[test]
    fn mailbox_never_holds_back_an_unstamped_producer() {
        let mailbox = TextureMailbox::new(4);
        mailbox.post(stamped(3, 1, 9));
        mailbox.post(message(3, 2));
        assert_eq!(parked_generation(&mailbox, 3), None);

        mailbox.post(message(3, 3));
        mailbox.post(stamped(3, 4, 1));
        assert_eq!(parked_generation(&mailbox, 3), Some(1));
    }

    #[test]
    fn only_two_stamped_messages_are_ordered() {
        assert!(supersedes(Some(2), Some(1)));
        assert!(!supersedes(Some(1), Some(2)));
        assert!(!supersedes(Some(1), Some(1)));
        assert!(!supersedes(None, Some(1)));
        assert!(!supersedes(Some(1), None));
        assert!(!supersedes(None, None));
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
