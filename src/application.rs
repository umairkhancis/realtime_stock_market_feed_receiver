//! **Ring 1 — application business rules.** What this program *does* with a
//! stream of datagrams, expressed without ever naming a socket, a file, or a
//! terminal.
//!
//! Each use case takes its collaborators as [`ports`] — traits declared here,
//! in the inner ring, and implemented out in [`crate::infrastructure`]. That is
//! the dependency inversion the layout exists for: `application` names
//! `DatagramSource`, never `UdpDatagramSource`, so the loopback adapter can be
//! replaced (or stubbed in a test) without a line of this layer changing.
//!
//! - [`ports`]     — the trait boundary: what the outside must provide.
//! - [`capture`]   — [`capture::Capture`], the arena of raw datagram payloads a
//!   run produces, and the mirror image of the transmitter's `EncodedFeed`.
//! - [`receive`]   — the `listen` command's capture use case, its configuration,
//!   and the transport report it produces.
//! - [`verify`]    — checking a capture against the transmitter's ground truth,
//!   and writing what arrived back out.
//! - [`slice_one`] — the single datagram of slice 1.
//!
//! There is deliberately no `summarise` use case. That command is a load
//! followed by a render, and wrapping `store.load()` in a function that adds
//! nothing would be ceremony, not architecture — the same judgement the
//! transmitter's `docs/clean_arch.md` records for its own `summary`. The
//! composition root calls the port directly, which is what a composition root
//! is for.

pub mod capture;
pub mod ports;
pub mod receive;
pub mod slice_one;
pub mod verify;

/// The crate's application-level result.
///
/// A boxed trait object rather than a concrete enum, on purpose. The Rust
/// Book's I/O-project chapter draws exactly this line: code consumed by callers
/// it cannot see owes them a matchable error type (which is why
/// [`crate::domain::codec::CodecError`] and
/// [`crate::infrastructure::csv::FeedError`] stay concrete enums), while an
/// application binary that only ever propagates errors to `main` and prints
/// them gains nothing from an enum it never matches on. The moment a second
/// consumer appears — an FFI boundary, a library split — this becomes an
/// `enum AppError` with `#[non_exhaustive]`, and the ports' associated `Error`
/// types are already the seam that makes that a local change.
///
/// The `T = ()` default keeps the common `Result` spelling short while leaving
/// `Result<CaptureOutcome>` available; the module-qualified
/// `application::Result` naming follows `io::Result` and `fmt::Result` rather
/// than inventing `AppResult`.
pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
