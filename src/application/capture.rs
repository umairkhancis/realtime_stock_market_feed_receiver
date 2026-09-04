//! [`Capture`] — the raw bytes a run actually pulled off the wire.
//!
//! The mirror image of the transmitter's `application::encoded_feed::
//! EncodedFeed`, and for the same reason: one contiguous arena plus an
//! `(offset, len)` index, so that nothing variable — no allocation, no `Vec`
//! header, no decode — happens inside the per-datagram budget. The capture loop
//! calls [`Capture::push`] and nothing else; [`Capture::decode_all`] runs after
//! the stream has ended.
//!
//! `bytes`, `index` and `arrivals` are private and the adapter fills them
//! through [`Capture::push`] rather than reaching across the ring boundary into
//! three fields ([C-STRUCT-PRIVATE]). That is also what keeps the three lengths
//! in step by construction.
//!
//! [C-STRUCT-PRIVATE]: https://rust-lang.github.io/api-guidelines/predictability.html#c-struct-private

use crate::domain::codec::{CodecError, MAX_MESSAGE_LEN, decode};
use crate::domain::message::ItchMessage;

/// Raw datagram payloads, kept as one arena rather than a `Vec<Vec<u8>>` so the
/// hot loop does no per-message allocation.
#[derive(Debug, Default)]
pub struct Capture {
    bytes: Vec<u8>,
    index: Vec<(u32, u8)>,
    /// Nanoseconds since the first datagram, one per captured datagram.
    arrivals: Vec<u64>,
}

impl Capture {
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn datagram(&self, i: usize) -> &[u8] {
        let (off, len) = self.index[i];
        &self.bytes[off as usize..off as usize + len as usize]
    }

    pub fn arrivals(&self) -> &[u64] {
        &self.arrivals
    }

    /// Payload bytes held so far, which is what a capture ceiling is measured
    /// against.
    pub fn payload_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Reserves room for `datagrams` more messages.
    ///
    /// One reserve up front beats a hundred thousand reallocations inside the
    /// loop, each of which is a memcpy of everything received so far.
    pub fn reserve(&mut self, datagrams: usize) {
        self.bytes.reserve(datagrams * MAX_MESSAGE_LEN);
        self.index.reserve(datagrams);
        self.arrivals.reserve(datagrams);
    }

    /// Records one datagram and when it arrived, in nanoseconds since the first.
    ///
    /// The whole of the hot loop's work on this type. Callers are expected to
    /// have already checked `payload.len() <= MAX_MESSAGE_LEN`, which the
    /// `as u8` below relies on.
    pub fn push(&mut self, payload: &[u8], arrival_nanos: u64) {
        self.index.push((self.bytes.len() as u32, payload.len() as u8));
        self.bytes.extend_from_slice(payload);
        self.arrivals.push(arrival_nanos);
    }

    /// Decodes everything captured. Returns the messages that decoded and, for
    /// each that did not, its index and why.
    pub fn decode_all(&self) -> (Vec<ItchMessage>, Vec<(usize, CodecError)>) {
        let mut msgs = Vec::with_capacity(self.len());
        let mut failures = Vec::new();
        for i in 0..self.len() {
            match decode(self.datagram(i)) {
                Ok(m) => msgs.push(m),
                Err(e) => failures.push((i, e)),
            }
        }
        (msgs, failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::codec::encode;
    use crate::domain::fixtures::synthetic;

    fn capture_of(msgs: &[ItchMessage]) -> Capture {
        let mut capture = Capture::default();
        let mut scratch = [0u8; MAX_MESSAGE_LEN];
        for m in msgs {
            let n = encode(m, &mut scratch).unwrap();
            capture.push(&scratch[..n], 0);
        }
        capture
    }

    #[test]
    fn a_capture_hands_back_exactly_the_bytes_it_was_given() {
        let msgs = synthetic(500);
        let capture = capture_of(&msgs);
        assert_eq!(capture.len(), 500);
        assert!(!capture.is_empty());

        let (decoded, failures) = capture.decode_all();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(decoded, msgs);
    }

    #[test]
    fn decode_all_reports_a_corrupt_datagram_without_losing_the_rest() {
        let msgs = synthetic(10);
        let mut capture = Capture::default();
        let mut scratch = [0u8; MAX_MESSAGE_LEN];
        for (i, m) in msgs.iter().enumerate() {
            let n = encode(m, &mut scratch).unwrap();
            if i == 4 {
                scratch[0] = b'Z'; // a type byte from a version we do not speak
            }
            capture.push(&scratch[..n], 0);
        }
        let (decoded, failures) = capture.decode_all();
        assert_eq!(decoded.len(), 9, "one bad datagram costs exactly one message");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, 4);
    }

    /// Reserving is an allocation hint and nothing more: it must not change
    /// what the capture holds or how it indexes.
    #[test]
    fn reserving_does_not_change_what_is_captured() {
        let msgs = synthetic(200);
        let mut reserved = Capture::default();
        reserved.reserve(200);
        let mut scratch = [0u8; MAX_MESSAGE_LEN];
        for (i, m) in msgs.iter().enumerate() {
            let n = encode(m, &mut scratch).unwrap();
            reserved.push(&scratch[..n], i as u64 * 100_000);
        }
        assert_eq!(reserved.decode_all().0, capture_of(&msgs).decode_all().0);
        assert_eq!(reserved.arrivals().len(), 200);
        assert_eq!(reserved.arrivals()[3], 300_000);
        assert_eq!(reserved.payload_bytes(), (0..200).map(|i| msgs[i].wire_len()).sum::<usize>());
    }

    #[test]
    fn an_empty_capture_decodes_to_nothing() {
        let capture = Capture::default();
        assert!(capture.is_empty());
        assert_eq!(capture.payload_bytes(), 0);
        assert_eq!(capture.decode_all().0.len(), 0);
    }
}
