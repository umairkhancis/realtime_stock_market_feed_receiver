//! **Ring 0 — enterprise rules.** What an ITCH feed *is*, and what it means for
//! one to be incomplete.
//!
//! Everything here would still be true if the feed arrived over TCP, were read
//! back from Parquet, or were never displayed at all. That is the test this
//! layer is held to, and it is enforced mechanically: no module below `domain`
//! may name `std::io`, `std::net`, `std::fs`, `std::thread`, `std::time`,
//! `println!`, or any third-party crate. See `tests/architecture.rs`.
//!
//! - [`message`] — the ITCH 5.0 records and the sum type the rest of the crate
//!   passes around. Byte-identical to the transmitter's copy, deliberately.
//! - [`codec`] — the byte layout those records take on the wire. Also
//!   byte-identical to the transmitter's.
//! - [`symbols`] — the locate → ticker map, including the rule for learning one
//!   from the add stream when no out-of-band file exists.
//! - [`loss`] — what a gap in an ITCH stream looks like from the inside, and
//!   what it looks like against an answer key.
//!
//! **Why the codec is domain and not infrastructure** is argued in the
//! transmitter's `docs/clean_arch.md` and applies here unchanged: ITCH's byte
//! layout *is* the product, it does not move when the transport does, and it
//! moves when NASDAQ revises the spec. The *transport* (UDP) and the *archive
//! format* (CSV) are the details, and both live outside.
//!
//! **Why loss detection is domain** is the receiver's own version of that
//! argument, and `docs/clean_arch.md` in this repo makes it.

pub mod codec;
pub mod loss;
pub mod message;
pub mod symbols;

#[cfg(test)]
pub mod fixtures;
