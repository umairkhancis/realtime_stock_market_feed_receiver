//! Dependency-free (std only) ITCH 5.0 receiver.
//!
//! The other half of the pipeline in the transmitter repo. Slice 1 received one
//! Add Order and printed it; slice 2 receives a paced 10,000 message/second
//! stream, one message per datagram, and answers the question that matters:
//! **did all of it arrive?**
//!
//! There are two ways to answer that, and they are not equivalent:
//!
//! - [`domain::loss::detect`] infers loss from the messages themselves — gaps in
//!   the order reference and match number sequences, dangling references in a
//!   replayed book. It needs nothing but the stream, which is what a real
//!   deployment has. It is also blind to roughly a third of the tape, and blind
//!   to tail loss.
//! - [`domain::loss::compare`] diffs what arrived against the transmitter's
//!   `feed.csv`. It has no blind spot at all, and it only works because this is
//!   a synthetic feed with an answer key.
//!
//! Both run, and the report says which is which. The gap between them is the
//! measure of what a session layer would buy — see `docs/session-layer.md` in
//! the transmitter repo.
//!
//! [`domain::codec`] and [`domain::message`] are byte-identical to the
//! transmitter's copies. That is deliberate: the golden vector is the wire
//! contract, and it means nothing if each side keeps its own idea of the
//! offsets.
//!
//! # Layout
//!
//! Four rings, dependencies pointing inward only. `docs/clean_arch.md` argues
//! every placement below and cites the Rust guidance behind it;
//! `tests/architecture.rs` enforces the arrows.
//!
//! ```text
//!   presentation ─┐                     cli, console, report, summary,
//!                 ├─→ application ─→ domain          format, banner
//!   infrastructure┘                      ↑     ports, use cases, capture
//!        csv, udp                        └── ITCH messages, codec, loss rules
//! ```
//!
//! - [`domain`] — what an ITCH feed *is*, and what it means for one to be
//!   incomplete. No I/O, no clock, no dependencies.
//! - [`application`] — what this program *does* with one, expressed over ports.
//! - [`infrastructure`] — the adapters that satisfy those ports: files and
//!   sockets.
//! - [`presentation`] — the terminal, and the composition root that wires the
//!   other three together.
//!
//! The rule that makes this more than folder-sorting: an inner ring may not
//! name an outer one. `application` declares
//! [`application::ports::DatagramSource`]; `infrastructure` implements it; only
//! [`presentation::cli`] knows that the implementation is a `UdpSocket`.

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod presentation;
