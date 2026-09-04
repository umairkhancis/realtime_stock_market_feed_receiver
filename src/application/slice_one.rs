//! Slice 1: one Add Order, alone, as the entire UDP payload.
//!
//! Kept as a use case rather than folded into the CLI so that the buffer-size
//! rule — a datagram that exactly fills the buffer may have been truncated by
//! the kernel, and the caller must be told — travels with the receive rather
//! than being re-derived by whoever prints it. The decode itself stays in the
//! composition root, because slice 1's whole output *is* the field-by-field
//! rendering of one message.

use crate::application::Result;
use crate::application::ports::DatagramListener;

/// Big enough that a jumbo-framed datagram still fits, so a full buffer
/// reliably means "something is wrong" rather than "this is a normal big one".
pub const RECV_BUF: usize = 2048;

/// One datagram, exactly as it came off the wire.
#[derive(Debug, Clone)]
pub struct OneShot {
    pub payload: Vec<u8>,
    pub from: String,
    /// The datagram filled the receive buffer, so the kernel may have cut it
    /// short. Not an error — the payload is still there, it just may not be all
    /// of it.
    pub truncated: bool,
}

/// Blocks for a single datagram.
pub fn receive_single(listener: &impl DatagramListener) -> Result<OneShot> {
    let mut buf = [0u8; RECV_BUF];
    let (n, from) = listener.receive_one(&mut buf)?;
    Ok(OneShot {
        // `&buf[..n]`, never `&buf` — the rest of the buffer is stale zeros.
        payload: buf[..n].to_vec(),
        from,
        truncated: n == buf.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    /// A listener that hands back a fixed payload — no socket, which is the
    /// point of the port.
    struct Canned(Vec<u8>);

    impl DatagramListener for Canned {
        type Error = Infallible;

        fn local_address(&self) -> std::result::Result<String, Infallible> {
            Ok("0.0.0.0:9000".into())
        }

        fn receive_one(&self, buf: &mut [u8]) -> std::result::Result<(usize, String), Infallible> {
            let n = self.0.len().min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            Ok((n, "127.0.0.1:54321".into()))
        }
    }

    #[test]
    fn a_short_datagram_is_returned_whole_and_not_flagged() {
        let one = receive_single(&Canned(vec![b'A'; 36])).unwrap();
        assert_eq!(one.payload.len(), 36);
        assert_eq!(one.from, "127.0.0.1:54321");
        assert!(!one.truncated);
    }

    /// A datagram that exactly fills the buffer is indistinguishable from one
    /// the kernel cut short, so it must be reported as suspect.
    #[test]
    fn a_buffer_filling_datagram_is_flagged_as_possibly_truncated() {
        let one = receive_single(&Canned(vec![0u8; RECV_BUF])).unwrap();
        assert_eq!(one.payload.len(), RECV_BUF);
        assert!(one.truncated);
    }
}
