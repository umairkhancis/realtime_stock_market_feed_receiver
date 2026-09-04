//! What it means for an ITCH stream to be incomplete.
//!
//! Two independent answers, kept in separate modules because they have
//! different powers and different blind spots, and conflating them is exactly
//! the mistake the report exists to prevent:
//!
//! - [`detect`] infers loss from the messages themselves — gaps in the order
//!   reference and match number sequences, dangling references in a replayed
//!   book. It needs nothing but the stream, which is what a real deployment
//!   has. It is also blind to roughly a third of the tape, and blind to tail
//!   loss.
//! - [`compare`] diffs what arrived against the transmitter's `feed.csv`. It
//!   has no blind spot at all, and it only works because this is a synthetic
//!   feed with an answer key.
//!
//! [`sequence`] is the arithmetic both of the first one's estimators are built
//! from, factored out because "what does a monotonic counter say about what is
//! missing from it" is a self-contained rule with its own failure modes.

pub mod compare;
pub mod detect;
pub mod sequence;

pub use compare::{Comparison, SCAN_WINDOW, compare};
pub use detect::{BookReport, Detection, LocateReport};
pub use sequence::SequenceGaps;
