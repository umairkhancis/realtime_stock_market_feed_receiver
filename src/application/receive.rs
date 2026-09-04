//! The `listen` command's capture use case, and the shape of what it reports.
//!
//! The socket is not here. What is here is the sequence a capture actually is —
//! pull the stream off a [`DatagramSource`], decode the arena, run the loss
//! detectors over what decoded — and the value types the run produces. That
//! sequence is the same whether the datagrams came from UDP, from a pcap
//! replay, or from a fake in a test, which is exactly the test for whether
//! something belongs in this ring.
//!
//! [`ArrivalStats`] and [`ReceiveReport`] live here rather than in the adapter
//! that fills them, for the reason the transmitter moved `PaceStats` inward:
//! they are part of what a *run reports*, not part of how a socket works.
//! `infrastructure` depending on `application` points inward and is fine; the
//! reverse would not have been.

use std::time::Duration;

use crate::application::Result;
use crate::application::ports::{CaptureObserver, DatagramSource};
use crate::domain::codec::CodecError;
use crate::domain::loss::Detection;
use crate::domain::message::ItchMessage;

#[derive(Debug, Clone)]
pub struct ReceiveConfig {
    pub port: u16,
    /// How long to wait for the *first* datagram. Generous, because the usual
    /// workflow is to start this side before the transmitter.
    pub startup_timeout: Duration,
    /// How long a silence ends the capture, once it has started.
    pub idle_timeout: Duration,
    /// Stop as soon as this many datagrams have arrived.
    pub expect: Option<u64>,
    /// Ceiling on captured payload bytes. Past it, counting continues but the
    /// bytes are dropped — a receiver that OOMs is worse than one that forgets.
    pub max_capture_bytes: usize,
    pub progress_every: Duration,
}

impl Default for ReceiveConfig {
    fn default() -> Self {
        ReceiveConfig {
            port: 9000,
            startup_timeout: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(2),
            expect: None,
            max_capture_bytes: 512 * 1024 * 1024,
            progress_every: Duration::from_secs(1),
        }
    }
}

/// Inter-arrival timing — how uniform the stream actually was on this side.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArrivalStats {
    pub gaps: u64,
    pub min: Duration,
    pub mean: Duration,
    pub p50: Duration,
    pub p99: Duration,
    pub max: Duration,
    pub span: Duration,
}

impl ArrivalStats {
    pub fn of(arrivals: &[u64]) -> ArrivalStats {
        if arrivals.len() < 2 {
            return ArrivalStats::default();
        }
        let mut deltas: Vec<u64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
        let span = Duration::from_nanos(arrivals[arrivals.len() - 1] - arrivals[0]);
        let sum: u128 = deltas.iter().map(|&d| d as u128).sum();
        let mean = Duration::from_nanos((sum / deltas.len() as u128) as u64);
        deltas.sort_unstable();
        let at = |q: f64| Duration::from_nanos(deltas[((deltas.len() - 1) as f64 * q) as usize]);
        ArrivalStats {
            gaps: deltas.len() as u64,
            min: Duration::from_nanos(deltas[0]),
            mean,
            p50: at(0.50),
            p99: at(0.99),
            max: Duration::from_nanos(deltas[deltas.len() - 1]),
            span,
        }
    }

    /// Datagrams per second across the capture.
    ///
    /// `gaps / span`, not `datagrams / span`: n datagrams spanning first to last
    /// cover n-1 intervals, and dividing by the wrong one reports 10,010/s for a
    /// stream that is exactly 10,000/s.
    pub fn rate(&self) -> f64 {
        let s = self.span.as_secs_f64();
        if s > 0.0 { self.gaps as f64 / s } else { 0.0 }
    }
}

#[derive(Debug, Clone)]
pub struct ReceiveReport {
    pub datagrams: u64,
    pub bytes: u64,
    /// Count per leading type byte, indexed by the byte itself.
    pub by_type: [u64; 256],
    /// Datagrams whose length did not match what their type byte implies.
    pub framing_errors: u64,
    /// Datagrams whose leading byte is not a type we know. These cannot even be
    /// length-checked, which on a framed stream would be unrecoverable — here it
    /// costs exactly one datagram, because one datagram is one message.
    pub unknown_types: u64,
    /// Datagrams that filled the receive buffer, so may have been truncated.
    pub oversize: u64,
    pub captured: usize,
    pub capture_truncated: bool,
    /// Who sent the first datagram, rendered by the adapter so this layer
    /// needs no `std::net` types. The report only ever prints it.
    pub first_peer: Option<String>,
    pub peers: u64,
    pub arrival: ArrivalStats,
}

impl Default for ReceiveReport {
    fn default() -> Self {
        ReceiveReport {
            datagrams: 0,
            bytes: 0,
            by_type: [0u64; 256],
            framing_errors: 0,
            unknown_types: 0,
            oversize: 0,
            captured: 0,
            capture_truncated: false,
            first_peer: None,
            peers: 0,
            arrival: ArrivalStats::default(),
        }
    }
}

impl ReceiveReport {
    pub fn known_types(&self) -> Vec<(u8, u64)> {
        let mut v: Vec<(u8, u64)> = (0..=255u8)
            .filter(|&t| self.by_type[t as usize] > 0)
            .map(|t| (t, self.by_type[t as usize]))
            .collect();
        v.sort_by_key(|&(t, _)| t);
        v
    }
}

/// Everything a capture produced, before anything has been rendered.
///
/// Returned whole rather than printed along the way. The three steps below are
/// separated by nothing but data, so the composition root can render between
/// them in whatever order it likes — which is how the `listen` output keeps its
/// shape without this layer knowing what that shape is.
#[derive(Debug)]
pub struct CaptureOutcome {
    pub report: ReceiveReport,
    pub messages: Vec<ItchMessage>,
    /// For each datagram that did not decode, its index and why.
    pub failures: Vec<(usize, CodecError)>,
    pub detection: Detection,
}

/// Captures a stream, decodes it, and infers what it lost.
///
/// Inference runs unconditionally and first, before any ground truth is
/// consulted, because it is what a real deployment would have. The comparison
/// in [`super::verify`] is the thing this feed can additionally afford.
pub fn capture_feed(
    source: &mut impl DatagramSource,
    config: &ReceiveConfig,
    observer: &mut dyn CaptureObserver,
) -> Result<CaptureOutcome> {
    let (capture, report) = source.capture(config, observer)?;
    let (messages, failures) = capture.decode_all();
    let detection = Detection::run(&messages);
    Ok(CaptureOutcome { report, messages, failures, detection })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrival_stats_on_a_perfectly_uniform_stream() {
        let arrivals: Vec<u64> = (0..1_000).map(|i| i * 100_000).collect();
        let s = ArrivalStats::of(&arrivals);
        assert_eq!(s.gaps, 999);
        assert_eq!(s.min, Duration::from_micros(100));
        assert_eq!(s.max, Duration::from_micros(100));
        assert_eq!(s.mean, Duration::from_micros(100));
        assert_eq!(s.p50, Duration::from_micros(100));
        assert_eq!(s.span, Duration::from_micros(99_900));
        // 1000 datagrams 100 µs apart span 99.9 ms and cover 999 intervals.
        // Dividing by the datagram count instead of the interval count would
        // report 10,010/s here.
        assert!((s.rate() - 10_000.0).abs() < 1e-6, "should read as 10k/s, got {}", s.rate());
    }

    #[test]
    fn arrival_stats_separate_the_typical_case_from_the_outlier() {
        let mut arrivals: Vec<u64> = (0..1_000).map(|i| i * 100_000).collect();
        // One 5 ms scheduler stall, of the kind that dominates `max` and says
        // nothing about the sender.
        for a in arrivals.iter_mut().skip(500) {
            *a += 5_000_000;
        }
        let s = ArrivalStats::of(&arrivals);
        assert_eq!(s.p50, Duration::from_micros(100), "the typical gap is unchanged");
        assert_eq!(s.max, Duration::from_micros(5_100));
        assert!(s.mean > s.p50, "one outlier drags the mean but not the median");
    }

    #[test]
    fn arrival_stats_are_empty_for_too_few_samples() {
        assert_eq!(ArrivalStats::of(&[]).gaps, 0);
        assert_eq!(ArrivalStats::of(&[42]).gaps, 0);
        assert_eq!(ArrivalStats::of(&[42]).rate(), 0.0);
    }

    /// The defaults the usage text promises.
    #[test]
    fn the_default_config_is_the_documented_one() {
        let cfg = ReceiveConfig::default();
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.startup_timeout, Duration::from_secs(60));
        assert_eq!(cfg.idle_timeout, Duration::from_millis(2_000));
        assert_eq!(cfg.expect, None);
        assert_eq!(cfg.progress_every, Duration::from_secs(1));
    }

    #[test]
    fn known_types_lists_only_what_arrived_in_byte_order() {
        let mut report = ReceiveReport::default();
        report.by_type[b'D' as usize] = 3;
        report.by_type[b'A' as usize] = 7;
        assert_eq!(report.known_types(), vec![(b'A', 7), (b'D', 3)]);
        assert!(ReceiveReport::default().known_types().is_empty());
    }
}
