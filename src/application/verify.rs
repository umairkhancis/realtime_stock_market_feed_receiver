//! Checking a capture against the transmitter's ground truth, and writing back
//! out what arrived.
//!
//! Both use cases are two lines of body, and both earn their place for the same
//! reason: they are the only two things in the program that pair a [`FeedStore`]
//! with a domain rule, and putting that pairing here is what keeps
//! `data/feed.csv` out of every layer but the composition root. Neither prints,
//! and neither knows whether the store is a file.

use crate::application::Result;
use crate::application::ports::{FeedStore, StoredFeed};
use crate::domain::loss::compare::{Comparison, compare};
use crate::domain::message::ItchMessage;

/// A capture measured against the answer key.
///
/// The two counts the report prints are already in
/// [`Comparison::expected`](crate::domain::loss::Comparison) and
/// `Comparison::received`, so this does not carry the loaded feed back out —
/// the ground truth is a few megabytes and the caller wants a verdict, not a
/// second copy of what it was verified against.
#[derive(Debug)]
pub struct Verification {
    /// Where the ground truth was read from, for the report line that says so.
    pub location: String,
    pub comparison: Comparison,
}

/// Loads the transmitter's feed and diffs an in-memory capture against it.
///
/// This is the check with no blind spot, and it is only possible because the
/// feed is synthetic and has an answer key. [`crate::domain::loss::detect`] is
/// the one that survives contact with production.
pub fn verify_against(truth: &impl FeedStore, received: &[ItchMessage]) -> Result<Verification> {
    let expected = truth.load()?;
    let comparison = compare(&expected, received);
    Ok(Verification { location: truth.location(), comparison })
}

/// The same check with both sides from storage, for the offline `verify`
/// command.
///
/// Loads the ground truth *before* the capture on purpose: an operator missing
/// both files should be told about the answer key first, since without it the
/// command cannot mean anything at all.
pub fn verify_stored(truth: &impl FeedStore, capture: &impl FeedStore) -> Result<Verification> {
    let expected = truth.load()?;
    let received = capture.load()?;
    let comparison = compare(&expected, &received);
    Ok(Verification { location: truth.location(), comparison })
}

/// Writes what arrived, in the transmitter's own format.
///
/// The point of the dump is that with zero loss it is byte-identical to
/// `feed.csv`, so plain `diff` is a complete check on its own. That property
/// belongs to the CSV adapter; all this use case knows is that a capture can be
/// stored.
pub fn dump_capture(store: &impl FeedStore, messages: &[ItchMessage]) -> Result<StoredFeed> {
    Ok(store.save(messages)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::StoredFeed;
    use crate::domain::fixtures::{drop_every, synthetic};
    use std::cell::RefCell;
    use std::convert::Infallible;

    /// A `FeedStore` with no filesystem behind it — which is the entire point
    /// of the port. Neither use case below opens a file.
    #[derive(Default)]
    struct MemoryStore {
        name: String,
        messages: Vec<ItchMessage>,
        saved: RefCell<Vec<ItchMessage>>,
    }

    impl MemoryStore {
        fn holding(name: &str, messages: Vec<ItchMessage>) -> Self {
            MemoryStore { name: name.into(), messages, saved: RefCell::new(Vec::new()) }
        }
    }

    impl FeedStore for MemoryStore {
        type Error = Infallible;

        fn location(&self) -> String {
            self.name.clone()
        }

        fn load(&self) -> std::result::Result<Vec<ItchMessage>, Infallible> {
            Ok(self.messages.clone())
        }

        fn save(&self, messages: &[ItchMessage]) -> std::result::Result<StoredFeed, Infallible> {
            *self.saved.borrow_mut() = messages.to_vec();
            Ok(StoredFeed { rows: messages.len() as u64, location: self.name.clone() })
        }
    }

    #[test]
    fn a_lossless_capture_verifies_and_names_its_ground_truth() {
        let sent = synthetic(2_000);
        let truth = MemoryStore::holding("truth.csv", sent.clone());

        let v = verify_against(&truth, &sent).unwrap();
        assert_eq!(v.location, "truth.csv");
        assert!(v.comparison.is_perfect());
        assert_eq!(v.comparison.expected, 2_000);
        assert_eq!(v.comparison.received, 2_000);
    }

    #[test]
    fn a_lossy_capture_fails_verification_with_the_counts_the_report_prints() {
        let sent = synthetic(2_000);
        let (kept, dropped) = drop_every(&sent, 50);
        let truth = MemoryStore::holding("truth.csv", sent);

        let v = verify_against(&truth, &kept).unwrap();
        assert!(!v.comparison.is_perfect());
        assert_eq!(v.comparison.missing.len(), dropped.len());
        assert_eq!(v.comparison.expected, 2_000);
        assert_eq!(v.comparison.received, kept.len() as u64);
    }

    /// The offline form reads both sides through ports and reaches the same
    /// verdict as the in-memory one.
    #[test]
    fn verifying_two_stored_feeds_agrees_with_verifying_one_in_memory() {
        let sent = synthetic(1_000);
        let (kept, _) = drop_every(&sent, 20);
        let truth = MemoryStore::holding("truth.csv", sent.clone());
        let capture = MemoryStore::holding("received.csv", kept.clone());

        let stored = verify_stored(&truth, &capture).unwrap();
        let in_memory = verify_against(&truth, &kept).unwrap();
        assert_eq!(stored.location, in_memory.location);
        assert_eq!(stored.comparison.missing, in_memory.comparison.missing);
        assert_eq!(stored.comparison.received, in_memory.comparison.received);
    }

    #[test]
    fn dumping_hands_the_store_exactly_what_arrived() {
        let msgs = synthetic(500);
        let dump = MemoryStore::holding("received.csv", Vec::new());

        let stored = dump_capture(&dump, &msgs).unwrap();
        assert_eq!(stored.rows, 500);
        assert_eq!(stored.location, "received.csv");
        assert_eq!(*dump.saved.borrow(), msgs);
    }
}
