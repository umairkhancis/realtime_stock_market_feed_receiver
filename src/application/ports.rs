//! The ports: what a use case needs from the world, stated as traits.
//!
//! Two shapes of dependency, dispatched differently on purpose — the same split
//! the transmitter makes, for the same reasons:
//!
//! - **Collaborators** ([`DatagramSource`], [`DatagramListener`], [`FeedStore`],
//!   [`SymbolSource`]) are taken as `impl Trait` by the use cases, so each call
//!   site monomorphizes and the abstraction costs nothing at run time. This is
//!   the Rust answer to "won't the indirection be slow?" — for static
//!   collaborators chosen once at the composition root, it is free, which
//!   matters more on this side than on the transmitter's: the capture loop is
//!   the one piece of this program with a hard real-time budget.
//! - **Output** ([`CaptureObserver`]) is taken as `&mut dyn`, because it is
//!   passed *through* the source into the capture loop. Making it generic would
//!   monomorphize the transport over the presenter for no benefit; the observer
//!   fires once per second, so one vtable hop is beneath measurement.
//!
//! Each port carries an associated `Error` type rather than boxing, following
//! the pattern of `FromStr`, `TryFrom` and `Iterator`: the adapter keeps its own
//! concrete error (`FeedError`, `io::Error`), and the `'static + Error` bound is
//! exactly what lets a use case turn it into [`super::Result`] with `?`.

use std::time::Duration;

use crate::application::capture::Capture;
use crate::application::receive::{ReceiveConfig, ReceiveReport};
use crate::domain::message::ItchMessage;
use crate::domain::symbols::SymbolMap;

/// Something that can capture a whole stream of datagrams until it goes quiet.
///
/// The boundary is drawn at the *whole run*, not at the individual datagram,
/// and that is a deliberate trade. Drawing it per-datagram would move the
/// receive loop up into the use case — the more textbook split — but the loop
/// also owns the socket's own read timeout, which is how end-of-stream is
/// detected, and one `Instant::now()` that has to be taken the moment
/// `recv_from` returns and not one syscall later. A per-datagram port would put
/// a trait call between the kernel and that timestamp, and would need a `Clock`
/// port beside it. Keeping the loop, the socket and the clock together in one
/// adapter keeps them cohesive; the use case still never names `std::net`,
/// which is the property that actually matters.
pub trait DatagramSource {
    type Error: std::error::Error + 'static;

    fn capture(
        &mut self,
        config: &ReceiveConfig,
        observer: &mut dyn CaptureObserver,
    ) -> Result<(Capture, ReceiveReport), Self::Error>;
}

/// A one-shot datagram receive, for slice 1's single message.
///
/// Separate from [`DatagramSource`] rather than a degenerate case of it,
/// because slice 1 is a different question: it wants the *bytes* of one
/// datagram to print as hex, not a capture arena and a transport report.
pub trait DatagramListener {
    type Error: std::error::Error + 'static;

    /// Rendered by the adapter, so this layer needs no `std::net` types.
    fn local_address(&self) -> Result<String, Self::Error>;

    /// Blocks until one datagram arrives. Returns how many bytes landed in
    /// `buf` and who sent them.
    fn receive_one(&self, buf: &mut [u8]) -> Result<(usize, String), Self::Error>;
}

/// Where a feed is read from and written to.
///
/// Deliberately says nothing about files. The use cases never see a path, which
/// is why swapping CSV for Parquet — or for an in-memory fake in a test — needs
/// no change in [`crate::application`]. The receiver holds two of these at
/// once: the transmitter's ground truth, and the dump of what arrived.
pub trait FeedStore {
    type Error: std::error::Error + 'static;

    /// Human-readable location, for output only. A path, a URL, a bucket key —
    /// the use case treats it as opaque and only ever prints it.
    fn location(&self) -> String;

    fn load(&self) -> Result<Vec<ItchMessage>, Self::Error>;

    fn save(&self, messages: &[ItchMessage]) -> Result<StoredFeed, Self::Error>;
}

/// What a [`FeedStore`] reports after a successful write.
#[derive(Debug, Clone)]
pub struct StoredFeed {
    pub rows: u64,
    pub location: String,
}

/// The out-of-band locate → ticker map, when there is one.
///
/// A separate port from [`FeedStore`] because the two are genuinely
/// independent here: `summary` reads its messages from the dump the receiver
/// wrote and its names from the map the *transmitter* wrote, and a store that
/// bundled the two would have to invent a symbols file beside `received.csv`
/// that nothing produces.
pub trait SymbolSource {
    type Error: std::error::Error + 'static;

    fn location(&self) -> String;

    fn load(&self) -> Result<SymbolMap, Self::Error>;
}

/// The output port: how a capture narrates itself while it is running.
///
/// Clean Architecture calls this a *presenter* — the use case pushes facts out
/// through it instead of returning a rendered string, so the capture loop can
/// report progress with no idea that stdout exists. That is not a stylistic
/// win here: printing inside the loop is one of the two documented ways to
/// measure your own receiver instead of the network (see
/// [`crate::infrastructure::net::udp`]), and a port is what makes "the loop
/// does not print" a structural fact rather than a comment.
///
/// Every method defaults to a no-op so a test implements the trait with an
/// empty `impl` block.
pub trait CaptureObserver {
    fn on_listening(&mut self, _start: &CaptureStart) {}
    fn on_progress(&mut self, _progress: &CaptureProgress) {}
}

/// Everything known at the instant the socket is bound and the wait begins.
#[derive(Debug, Clone)]
pub struct CaptureStart {
    /// Rendered by the adapter, so this layer needs no `std::net` types.
    pub local: String,
    pub startup_timeout: Duration,
    pub idle_timeout: Duration,
}

/// A periodic snapshot of a capture in flight.
#[derive(Debug, Clone, Copy)]
pub struct CaptureProgress {
    pub elapsed: Duration,
    pub datagrams: u64,
    pub bytes: u64,
}

/// A [`CaptureObserver`] that says nothing. Useful in tests, and as the default
/// when `progress_every` is zero.
#[derive(Debug, Clone, Copy, Default)]
pub struct SilentObserver;

impl CaptureObserver for SilentObserver {}
