//! **Ring 3 — frameworks and drivers.** How the outside world is actually
//! touched.
//!
//! Every adapter here implements a port declared in [`crate::application`], and
//! is chosen by the composition root in [`crate::presentation::cli`]. Nothing
//! in `domain` or `application` names a module in this layer; the dependency
//! arrows all point inward, which is what makes the loopback test in
//! [`net::udp`] a test of one adapter rather than a test of the whole program.
//!
//! - [`csv`] — the on-disk feed format, and the
//!   [`crate::application::ports::FeedStore`] and
//!   [`crate::application::ports::SymbolSource`] adapters over the filesystem.
//! - [`net`] — the bind address policy, and the two `UdpSocket` adapters: the
//!   capture loop behind [`crate::application::ports::DatagramSource`] and
//!   slice 1's one-shot behind [`crate::application::ports::DatagramListener`].
//!
//! `presentation` is this layer's sibling, not its parent. Neither may name the
//! other, which is why the capture loop reports progress through an observer
//! instead of calling `println!`.

pub mod csv;
pub mod net;
