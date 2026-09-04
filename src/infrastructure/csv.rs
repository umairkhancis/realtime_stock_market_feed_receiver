//! CSV: the on-disk shape of a feed, and the filesystem adapters over it.
//!
//! Split in two so that the *format* can be exercised without touching a disk:
//!
//! - [`serde`] reads and writes over any `BufRead`/`Write`, so its tests round
//!   trip through a `Vec<u8>` and never create a file. That follows the API
//!   guidelines' [C-GENERIC] advice to accept `impl Write` rather than a
//!   concrete `File`, and it is what lets the receiver's central claim — a
//!   lossless capture dumps a byte-identical file — be a unit test.
//! - [`store`] is the thin part that knows about paths, directories and
//!   `File`, and the only thing that implements the two storage ports.
//!
//! CSV is the archetypal *detail*: swap it for Parquet and not one ITCH message
//! changes. That is why it sits out here and the wire codec does not.
//!
//! [C-GENERIC]: https://rust-lang.github.io/api-guidelines/flexibility.html

pub mod serde;
pub mod store;

pub use serde::{FeedError, HEADER, read_feed, read_symbol_table, write_feed};
pub use store::{CsvFeedStore, CsvSymbolSource};
