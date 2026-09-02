//! The capture loop.
//!
//! The transmitter's job is to hold a 100 µs schedule; this side's job is to
//! never be the reason a datagram goes missing. Those are the same discipline
//! seen from opposite ends, and it comes down to one rule: **do as little as
//! possible per datagram, and do the rest afterwards.**
//!
//! Concretely, the loop does four things — `recv_from`, one `Instant::now()`,
//! a length check against the type byte, and an `extend_from_slice` into a
//! preallocated arena. It does not decode, does not allocate per message, and
//! above all does not print. Decoding, detection and comparison all run after
//! the stream has ended, over the captured bytes.
//!
//! Two ways to accidentally measure your own receiver instead of the network:
//!
//! - **Printing per datagram.** 100,000 `println!` to a terminal, at a 100 µs
//!   budget each, makes the terminal emulator the bottleneck. The socket buffer
//!   overflows and the loss looks like the network's fault. There is a progress
//!   line here, but it fires on a timer — ten lines for a ten-second run.
//! - **The socket receive buffer.** `net.inet.udp.recvspace` defaults to about
//!   786 KB on macOS, and `std::net::UdpSocket` exposes no `SO_RCVBUF` setter —
//!   raising it needs `libc`, which the brief rules out. That is headroom for a
//!   few thousand datagrams, a few hundred milliseconds at the slice-2 rate:
//!   ample for a loop that only counts, nowhere near enough for one that works.
//!   The fix is to not fall behind, not to buffer more.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::codec::{decode, wire_len, CodecError, MAX_MESSAGE_LEN};
use crate::model::ItchMessage;

/// Big enough that a jumbo-framed datagram still fits, so `n == buf.len()`
/// reliably means "something is wrong" rather than "this is a normal big one".
const RECV_BUF: usize = 2048;

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
    pub first_peer: Option<SocketAddr>,
    pub peers: u64,
    pub arrival: ArrivalStats,
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

/// Binds, receives until the stream goes quiet, and returns what arrived.
pub fn receive(cfg: &ReceiveConfig) -> io::Result<(Capture, ReceiveReport)> {
    // 0.0.0.0, not 127.0.0.1: binding loopback works perfectly on this machine
    // and silently receives nothing from another one.
    let sock = UdpSocket::bind(("0.0.0.0", cfg.port))?;
    println!("listening on {}", sock.local_addr()?);
    println!(
        "waiting up to {:?} for the first datagram, then ending after {:?} of silence",
        cfg.startup_timeout, cfg.idle_timeout
    );
    sock.set_read_timeout(Some(cfg.startup_timeout))?;

    let mut capture = Capture::default();
    if let Some(n) = cfg.expect {
        // One reserve up front beats a hundred thousand reallocations inside the
        // loop, each of which is a memcpy of everything received so far.
        capture.bytes.reserve(n as usize * MAX_MESSAGE_LEN);
        capture.index.reserve(n as usize);
        capture.arrivals.reserve(n as usize);
    }

    let mut report = ReceiveReport {
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
    };

    let mut buf = [0u8; RECV_BUF];
    let mut origin: Option<Instant> = None;
    let mut next_progress = cfg.progress_every;

    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let now = Instant::now();
                let start = *origin.get_or_insert_with(|| {
                    // First datagram: switch to the tighter idle timeout and
                    // note who is sending.
                    let _ = sock.set_read_timeout(Some(cfg.idle_timeout));
                    now
                });
                match report.first_peer {
                    None => {
                        report.first_peer = Some(from);
                        report.peers = 1;
                    }
                    Some(p) if p != from => report.peers += 1,
                    _ => {}
                }

                report.datagrams += 1;
                report.bytes += n as u64;
                if n == buf.len() {
                    report.oversize += 1;
                }

                let type_byte = if n > 0 { buf[0] } else { 0 };
                report.by_type[type_byte as usize] += 1;
                match wire_len(type_byte) {
                    None => report.unknown_types += 1,
                    Some(expected) if expected != n => report.framing_errors += 1,
                    Some(_) => {}
                }

                if capture.bytes.len() + n <= cfg.max_capture_bytes && n <= MAX_MESSAGE_LEN {
                    capture.index.push((capture.bytes.len() as u32, n as u8));
                    capture.bytes.extend_from_slice(&buf[..n]);
                    capture.arrivals.push((now - start).as_nanos() as u64);
                } else {
                    report.capture_truncated = true;
                }

                if !cfg.progress_every.is_zero() {
                    let elapsed = now - start;
                    if elapsed >= next_progress {
                        println!(
                            "  {:>5.1}s  {:>7} datagrams  {:>8.0}/s  {:>10} bytes",
                            elapsed.as_secs_f64(),
                            report.datagrams,
                            report.datagrams as f64 / elapsed.as_secs_f64(),
                            report.bytes,
                        );
                        next_progress = elapsed + cfg.progress_every;
                    }
                }

                if cfg.expect.is_some_and(|want| report.datagrams >= want) {
                    break;
                }
            }
            // A fired read timeout is `WouldBlock` on Unix and `TimedOut` on
            // Windows. Matching only one of them turns a normal end-of-stream
            // into a crash on the other platform.
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                if origin.is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "no datagram arrived within {:?} — is the transmitter running, \
                             and pointed at this host's port {}?",
                            cfg.startup_timeout, cfg.port
                        ),
                    ));
                }
                break;
            }
            Err(e) => return Err(e),
        }
    }

    report.captured = capture.len();
    report.arrival = ArrivalStats::of(&capture.arrivals);
    Ok((capture, report))
}

pub fn print_report(report: &ReceiveReport) {
    println!();
    println!("== transport ==");
    println!("  datagrams       {}", report.datagrams);
    println!(
        "  payload         {} bytes, {:.1} bytes/datagram average",
        report.bytes,
        report.bytes as f64 / report.datagrams.max(1) as f64
    );
    if let Some(p) = report.first_peer {
        println!("  source          {p}");
    }
    if report.peers > 1 {
        println!("  WARNING         {} different sources sent to this port", report.peers);
    }
    println!("  framing         {} length mismatches, {} unknown type bytes",
        report.framing_errors, report.unknown_types);
    if report.oversize > 0 {
        println!("  WARNING         {} datagrams filled the receive buffer and may be truncated",
            report.oversize);
    }
    if report.capture_truncated {
        println!("  WARNING         capture ceiling hit — counts are complete, contents are not");
    }

    let a = &report.arrival;
    if a.gaps > 0 {
        println!();
        println!("== arrival timing ==");
        println!("  span            {:.3}s -> {:.0} datagrams/s", a.span.as_secs_f64(), a.rate());
        println!(
            "  inter-arrival   min {:>8.1}µs  p50 {:>8.1}µs  mean {:>8.1}µs  p99 {:>8.1}µs  max {:>9.1}µs",
            a.min.as_nanos() as f64 / 1000.0,
            a.p50.as_nanos() as f64 / 1000.0,
            a.mean.as_nanos() as f64 / 1000.0,
            a.p99.as_nanos() as f64 / 1000.0,
            a.max.as_nanos() as f64 / 1000.0,
        );
        println!("  (the transmitter paces at 100.0µs; p50 is the honest measure of whether it held,");
        println!("   and max is dominated by this machine's scheduler, not by the sender)");
    }
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

    #[test]
    fn a_capture_hands_back_exactly_the_bytes_it_was_given() {
        let msgs = crate::fixtures::synthetic(500);
        let mut capture = Capture::default();
        let mut scratch = [0u8; MAX_MESSAGE_LEN];
        for m in &msgs {
            let n = crate::codec::encode(m, &mut scratch).unwrap();
            capture.index.push((capture.bytes.len() as u32, n as u8));
            capture.bytes.extend_from_slice(&scratch[..n]);
        }
        assert_eq!(capture.len(), 500);

        let (decoded, failures) = capture.decode_all();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(decoded, msgs);
    }

    #[test]
    fn decode_all_reports_a_corrupt_datagram_without_losing_the_rest() {
        let msgs = crate::fixtures::synthetic(10);
        let mut capture = Capture::default();
        let mut scratch = [0u8; MAX_MESSAGE_LEN];
        for (i, m) in msgs.iter().enumerate() {
            let n = crate::codec::encode(m, &mut scratch).unwrap();
            if i == 4 {
                scratch[0] = b'Z'; // a type byte from a version we do not speak
            }
            capture.index.push((capture.bytes.len() as u32, n as u8));
            capture.bytes.extend_from_slice(&scratch[..n]);
        }
        let (decoded, failures) = capture.decode_all();
        assert_eq!(decoded.len(), 9, "one bad datagram costs exactly one message");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, 4);
    }

    /// End to end over loopback, against a real socket.
    #[test]
    fn receives_a_real_stream_and_measures_it() {
        let cfg = ReceiveConfig {
            port: 0, // let the kernel choose, so parallel tests cannot collide
            startup_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_millis(300),
            expect: None,
            progress_every: Duration::ZERO,
            ..Default::default()
        };

        // `receive` binds internally, so bind here first to learn a free port,
        // then hand it over.
        let probe = UdpSocket::bind(("0.0.0.0", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let msgs = crate::fixtures::synthetic(300);
        let sender = std::thread::spawn(move || {
            let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
            // Give the receiver a moment to bind before the first datagram.
            std::thread::sleep(Duration::from_millis(150));
            let mut scratch = [0u8; MAX_MESSAGE_LEN];
            for m in &crate::fixtures::synthetic(300) {
                let n = crate::codec::encode(m, &mut scratch).unwrap();
                sock.send_to(&scratch[..n], ("127.0.0.1", port)).unwrap();
                std::thread::sleep(Duration::from_micros(200));
            }
        });

        let (capture, report) = receive(&ReceiveConfig { port, ..cfg }).unwrap();
        sender.join().unwrap();

        assert_eq!(report.framing_errors, 0);
        assert_eq!(report.unknown_types, 0);
        assert_eq!(report.oversize, 0);
        assert_eq!(report.peers, 1);
        assert!(report.datagrams >= 290, "only {} of 300 arrived", report.datagrams);
        assert_eq!(report.captured as u64, report.datagrams);

        let (decoded, failures) = capture.decode_all();
        assert!(failures.is_empty());
        // Loopback does not reorder, so what arrived is a prefix-preserving
        // subsequence of what was sent.
        let c = crate::compare::compare(&msgs, &decoded);
        assert!(c.unexpected.is_empty(), "nothing foreign should appear: {:?}", c.unexpected);
        assert_eq!(c.matched, decoded.len() as u64);
    }
}
