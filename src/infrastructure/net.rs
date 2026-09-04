//! The network adapters, and the one piece of addressing policy they share.
//!
//! [`DEFAULT_PORT`] lives here rather than in [`crate::application`] because it
//! is a property of how this program is *deployed on a socket*, not of what a
//! capture is. A use case that knew the number would be a use case that could
//! not be run against a pcap replay.

pub mod udp;

/// The port both commands bind when the operator names nothing else.
///
/// Matches the transmitter's own default destination port, so `tx transmit`
/// and `rx listen` line up with no arguments on either side.
pub const DEFAULT_PORT: u16 = 9000;

/// The address to bind.
///
/// `0.0.0.0`, not `127.0.0.1`: binding loopback works perfectly on this machine
/// and silently receives nothing from another one. That is the single most
/// expensive mistake available on this side of the pipeline, so it is stated
/// once, here, rather than spelled out at each of the two bind sites.
pub const BIND_ADDRESS: &str = "0.0.0.0";
