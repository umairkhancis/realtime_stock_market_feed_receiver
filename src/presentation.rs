//! **Ring 3 — interface adapters for the human.** Everything that writes to a
//! terminal, and the only layer permitted to name a third-party crate.
//!
//! - [`cli`] — the composition root's controller: usage text, command dispatch,
//!   and the default paths and ports an operator gets when they pass nothing.
//! - [`console`] — [`console::ConsoleObserver`], which implements
//!   [`crate::application::ports::CaptureObserver`] so the capture loop can
//!   narrate itself without knowing stdout exists, plus the transport report.
//! - [`report`] — the two loss reports, rendered. Ordered so the caveat cannot
//!   be missed: what was proven, then what could not be.
//! - [`format`] — pure rendering helpers: hex dumps and scaled prices.
//! - [`summary`] — the feed description the `summary` command prints.
//! - [`banner`] — the figlet/colour splash. `figlet-rs` and `colored` are
//!   reachable from this module and nowhere else in the crate, which is the
//!   whole point of putting it here.
//!
//! `infrastructure` is this layer's sibling, not its child: dispatch here may
//! name an adapter, but no adapter may name anything in this module.

pub mod banner;
pub mod cli;
pub mod console;
pub mod format;
pub mod report;
pub mod summary;
