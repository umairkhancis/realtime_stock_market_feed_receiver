//! Message-for-message comparison against the transmitter's CSV.
//!
//! Everything in [`super::detect`] is inference — it works with nothing but the
//! stream, which is what a real deployment has, and it is blind to roughly a
//! third of the tape. This module is the opposite: given `feed.csv`, it says
//! exactly which messages did not arrive. No inference, no blind spot.
//!
//! That makes it the definitive check for slice 2 and useless in production,
//! which is the correct division of labour: use it to establish that the
//! transport works, use the detectors to monitor it once it does.
//!
//! The matching is a forward scan, not a set intersection, because order
//! matters — a stream that arrives complete but shuffled is a different failure
//! from a stream that arrives short, and a set would call them both fine. It
//! relies on messages being distinguishable, which they are: the transmitter
//! stamps every message with a distinct timestamp, so no two rows of a feed are
//! byte-identical.

use crate::domain::message::ItchMessage;

/// How far ahead of the cursor to look for a match before giving up and calling
/// a message unexpected. Sized to survive a burst loss of ~65k messages —
/// 6.5 seconds at the slice-2 rate — without losing the thread.
pub const SCAN_WINDOW: usize = 65_536;

#[derive(Debug, Clone, Default)]
pub struct Comparison {
    pub expected: u64,
    pub received: u64,
    pub matched: u64,
    /// Indices into the expected feed that never arrived.
    pub missing: Vec<u64>,
    /// Indices into the received stream that matched nothing ahead of the
    /// cursor: corruption, foreign traffic on the port, or reordering beyond
    /// [`SCAN_WINDOW`].
    pub unexpected: Vec<u64>,
}

impl Comparison {
    pub fn is_perfect(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty() && self.expected == self.received
    }

    pub fn loss_fraction(&self) -> f64 {
        if self.expected == 0 {
            0.0
        } else {
            self.missing.len() as f64 / self.expected as f64
        }
    }

    /// True when everything missing sits at the end — the signature of a sender
    /// that stopped early, or of tail loss, as opposed to drops scattered
    /// through the run.
    pub fn is_tail_truncation(&self) -> bool {
        match self.missing.first() {
            None => false,
            Some(&first) => {
                let n = self.missing.len() as u64;
                first + n == self.expected
                    && self.missing.windows(2).all(|w| w[1] == w[0] + 1)
            }
        }
    }

    /// Runs of consecutive missing indices, as `(start, length)` — a burst loss
    /// of 400 reads very differently from 400 independent drops.
    pub fn gap_runs(&self) -> Vec<(u64, u64)> {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for &i in &self.missing {
            match runs.last_mut() {
                Some((start, len)) if *start + *len == i => *len += 1,
                _ => runs.push((i, 1)),
            }
        }
        runs
    }

    pub fn largest_gap(&self) -> u64 {
        self.gap_runs().into_iter().map(|(_, len)| len).max().unwrap_or(0)
    }
}

/// Compares what arrived against what was sent.
pub fn compare(expected: &[ItchMessage], received: &[ItchMessage]) -> Comparison {
    let mut c = Comparison {
        expected: expected.len() as u64,
        received: received.len() as u64,
        ..Default::default()
    };

    let mut cursor = 0usize;
    for (r, msg) in received.iter().enumerate() {
        let limit = (cursor + SCAN_WINDOW).min(expected.len());
        match expected[cursor..limit].iter().position(|e| e == msg) {
            Some(offset) => {
                let hit = cursor + offset;
                // Everything skipped over was sent and did not arrive.
                c.missing.extend((cursor as u64)..(hit as u64));
                c.matched += 1;
                cursor = hit + 1;
            }
            None => c.unexpected.push(r as u64),
        }
    }
    // Whatever is left after the last match was never received.
    c.missing.extend((cursor as u64)..(expected.len() as u64));
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fixtures::{drop_every, drop_indices, synthetic};

    #[test]
    fn a_complete_capture_is_perfect() {
        let sent = synthetic(5_000);
        let c = compare(&sent, &sent);
        assert!(c.is_perfect());
        assert_eq!(c.matched, 5_000);
        assert_eq!(c.loss_fraction(), 0.0);
        assert_eq!(c.largest_gap(), 0);
        assert!(!c.is_tail_truncation());
    }

    /// The definitive property: it names the exact indices, including the `D`
    /// and `X` messages that [`super::detect`] cannot see at all.
    #[test]
    fn it_names_every_missing_message_including_the_invisible_ones() {
        let sent = synthetic(5_000);
        let victims: Vec<usize> = (0..sent.len())
            .filter(|&i| matches!(sent[i].message_type(), b'D' | b'X'))
            .take(50)
            .collect();
        let got = drop_indices(&sent, &victims);

        let c = compare(&sent, &got);
        assert_eq!(c.missing, victims.iter().map(|&i| i as u64).collect::<Vec<_>>());
        assert!(c.unexpected.is_empty());
        assert_eq!(c.matched, got.len() as u64);

        // The contrast that justifies this module existing at all.
        assert_eq!(crate::domain::loss::detect::Detection::run(&got).provable_loss(), 0);
    }

    #[test]
    fn scattered_loss_is_counted_and_located() {
        let sent = synthetic(10_000);
        let (got, dropped) = drop_every(&sent, 20);
        let c = compare(&sent, &got);
        assert_eq!(c.missing.len(), dropped.len());
        assert_eq!(c.missing, dropped.iter().map(|&i| i as u64).collect::<Vec<_>>());
        assert!((c.loss_fraction() - 0.05).abs() < 1e-9);
        assert_eq!(c.largest_gap(), 1, "one-at-a-time loss is 500 runs of length 1");
        assert_eq!(c.gap_runs().len(), 500);
    }

    #[test]
    fn a_burst_loss_is_reported_as_one_run() {
        let sent = synthetic(5_000);
        let victims: Vec<usize> = (1_000..1_400).collect();
        let c = compare(&sent, &drop_indices(&sent, &victims));
        assert_eq!(c.missing.len(), 400);
        assert_eq!(c.gap_runs(), vec![(1_000, 400)]);
        assert_eq!(c.largest_gap(), 400);
        assert!(!c.is_tail_truncation());
    }

    /// Tail loss — the case every content-based detector is blind to — is
    /// obvious here, because the answer key says how many there should have been.
    #[test]
    fn tail_truncation_is_identified_as_such() {
        let sent = synthetic(5_000);
        let c = compare(&sent, &sent[..4_500]);
        assert_eq!(c.missing.len(), 500);
        assert!(c.is_tail_truncation());
        assert_eq!(c.gap_runs(), vec![(4_500, 500)]);
        assert!(crate::domain::loss::detect::Detection::run(&sent[..4_500]).timestamps.is_clean());
    }

    #[test]
    fn foreign_traffic_is_flagged_rather_than_counted_as_a_match() {
        let sent = synthetic(1_000);
        let mut got = sent.clone();
        // A message that was never sent — say, another feed on the same port.
        got.insert(500, synthetic(2_000)[1_999]);
        let c = compare(&sent, &got);
        assert_eq!(c.unexpected, vec![500]);
        assert_eq!(c.matched, 1_000);
        assert!(c.missing.is_empty());
        assert!(!c.is_perfect(), "received != expected count");
    }

    #[test]
    fn an_empty_capture_reports_total_loss() {
        let sent = synthetic(100);
        let c = compare(&sent, &[]);
        assert_eq!(c.missing.len(), 100);
        assert_eq!(c.matched, 0);
        assert!(c.is_tail_truncation(), "receiving nothing is truncation at index 0");
        assert!((c.loss_fraction() - 1.0).abs() < 1e-9);

        // And the degenerate other way round.
        let c = compare(&[], &[]);
        assert!(c.is_perfect());
        assert_eq!(c.loss_fraction(), 0.0);
    }
}
