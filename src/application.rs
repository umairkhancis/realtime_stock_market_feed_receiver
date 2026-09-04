//! **Application layer** — what this program is *for*.
//!
//! Slice 2 asks one question of a 100,000-message capture: did all of it
//! arrive? These three modules are the answer, and they are ordered by how much
//! they are allowed to assume:
//!
//! - [`detect`] infers loss from the stream alone — gaps in order references
//!   and match numbers, dangling references in a replayed book. It assumes
//!   nothing a production deployment would not have, and pays for that with a
//!   blind spot it declares.
//! - [`compare`] diffs the capture against the transmitter's `feed.csv`. No
//!   blind spot, and no use outside a synthetic feed with an answer key.
//! - [`summary`] describes what arrived, so two runs can be compared by shape
//!   rather than by count.
//!
//! All three take `&[ItchMessage]` — already decoded, already in memory. None
//! of them knows where the messages came from, which is the point: the same
//! detectors run over a live capture, a replayed CSV and a test fixture.

pub mod compare;
pub mod detect;
pub mod summary;
