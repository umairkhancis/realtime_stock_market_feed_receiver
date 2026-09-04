//! **Presentation layer** — rendering, and the only home for a third-party crate.
//!
//! [`formatter`] holds the display primitives — a hex dump for eyeballing a
//! payload against `tcpdump -X`, ITCH's scale-by-10,000 prices as decimal
//! strings, and the startup banner. Integer maths stays inwards in the domain;
//! turning it into characters happens here.
//!
//! `figlet-rs` and `colored` are named in this module and nowhere else in the
//! crate. That containment is deliberate: the receiver's core is std-only, and
//! keeping the dependency at the outermost edge is what keeps that claim true.

pub mod formatter;
