//! **Domain layer** — the ITCH 5.0 protocol itself.
//!
//! The innermost circle: entities and the wire contract that defines them.
//! Nothing here opens a socket, touches the filesystem, prints, or names a
//! third-party crate — it depends on `core`/`std` primitives and on itself.
//! Every other layer may point inwards at this one; this one points at nobody.
//!
//! [`codec`] lives here rather than in `infrastructure` because it is not an
//! adapter to an external system: it is the specification. The byte offsets are
//! the rule that makes an [`model::ItchMessage`] mean anything, and they are
//! byte-identical to the transmitter's copy for exactly that reason. It is also
//! total, pure and allocation-free, which is what tells you it is a rule and
//! not a device.

pub mod codec;
pub mod model;
