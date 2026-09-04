//! **Infrastructure layer** — the two devices this program talks to.
//!
//! [`receive`] is the UDP socket; [`feed`] is the filesystem, in the
//! transmitter's CSV format. Both are adapters: they turn an external
//! representation into [`crate::domain::model::ItchMessage`] values and back,
//! and they are the only modules in the crate that can block, fail on `errno`,
//! or care what a byte was before it was a message.
//!
//! Both are also interchangeable by construction, which is what makes the
//! `verify` command possible: it replays two CSVs through the same comparison
//! the live capture uses, with no network in the picture at all.

pub mod feed;
pub mod receive;
