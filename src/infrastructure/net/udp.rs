//! The capture loop, and slice 1's one-shot receive.
//!
//! The transmitter's job is to hold a 100 µs schedule; this side's job is to
//! never be the reason a datagram goes missing. Those are the same discipline
//! seen from opposite ends, and it comes down to one rule: **do as little as
//! possible per datagram, and do the rest afterwards.**
//!
//! Concretely, the loop does four things — `recv_from`, one `Instant::now()`,
//! a length check against the type byte, and one `Capture::push` into a
//! preallocated arena. It does not decode, does not allocate per message, and
//! above all does not print. Decoding, detection and comparison all run after
//! the stream has ended, in [`crate::application::receive::capture_feed`], over
//! the captured bytes.
//!
//! Two ways to accidentally measure your own receiver instead of the network:
//!
//! - **Printing per datagram.** 100,000 `println!` to a terminal, at a 100 µs
//!   budget each, makes the terminal emulator the bottleneck. The socket buffer
//!   overflows and the loss looks like the network's fault. There is a progress
//!   line here, but it fires on a timer — ten lines for a ten-second run — and
//!   it goes out through
//!   [`CaptureObserver`](crate::application::ports::CaptureObserver) rather than
//!   to stdout, so this file cannot print even by accident.
//! - **The socket receive buffer.** `net.inet.udp.recvspace` defaults to about
//!   786 KB on macOS, and `std::net::UdpSocket` exposes no `SO_RCVBUF` setter —
//!   raising it needs `libc`, which the brief rules out. That is headroom for a
//!   few thousand datagrams, a few hundred milliseconds at the slice-2 rate:
//!   ample for a loop that only counts, nowhere near enough for one that works.
//!   The fix is to not fall behind, not to buffer more.
//!
//! The port boundary is the whole run rather than the single datagram, and
//! [`crate::application::ports::DatagramSource`] argues why: a per-datagram
//! trait call would sit between the kernel and the arrival timestamp.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

use crate::application::capture::Capture;
use crate::application::ports::{
    CaptureObserver, CaptureProgress, CaptureStart, DatagramListener, DatagramSource,
};
use crate::application::receive::{ArrivalStats, ReceiveConfig, ReceiveReport};
use crate::domain::codec::{MAX_MESSAGE_LEN, wire_len};
use crate::infrastructure::net::BIND_ADDRESS;

/// Big enough that a jumbo-framed datagram still fits, so `n == buf.len()`
/// reliably means "something is wrong" rather than "this is a normal big one".
const RECV_BUF: usize = 2048;

/// The [`DatagramSource`] adapter: binds a `UdpSocket` and captures until the
/// stream goes quiet.
#[derive(Debug, Clone, Copy, Default)]
pub struct UdpDatagramSource;

impl UdpDatagramSource {
    pub fn new() -> Self {
        UdpDatagramSource
    }
}

impl DatagramSource for UdpDatagramSource {
    type Error = io::Error;

    fn capture(
        &mut self,
        cfg: &ReceiveConfig,
        observer: &mut dyn CaptureObserver,
    ) -> io::Result<(Capture, ReceiveReport)> {
        receive(cfg, observer)
    }
}

/// Binds, receives until the stream goes quiet, and returns what arrived.
fn receive(
    cfg: &ReceiveConfig,
    observer: &mut dyn CaptureObserver,
) -> io::Result<(Capture, ReceiveReport)> {
    let sock = UdpSocket::bind((BIND_ADDRESS, cfg.port))?;
    observer.on_listening(&CaptureStart {
        local: sock.local_addr()?.to_string(),
        startup_timeout: cfg.startup_timeout,
        idle_timeout: cfg.idle_timeout,
    });
    sock.set_read_timeout(Some(cfg.startup_timeout))?;

    let mut capture = Capture::default();
    if let Some(n) = cfg.expect {
        capture.reserve(n as usize);
    }

    let mut report = ReceiveReport::default();

    let mut buf = [0u8; RECV_BUF];
    let mut origin: Option<Instant> = None;
    let mut next_progress = cfg.progress_every;
    // Kept as a `SocketAddr` for the length of the loop and rendered once at
    // the end. `ReceiveReport` carries a `String` so that `application` need
    // not name `std::net`, but stringifying per datagram to compare peers
    // would put an allocation inside the budget.
    let mut first_peer: Option<SocketAddr> = None;

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
                match first_peer {
                    None => {
                        first_peer = Some(from);
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

                if capture.payload_bytes() + n <= cfg.max_capture_bytes && n <= MAX_MESSAGE_LEN {
                    capture.push(&buf[..n], (now - start).as_nanos() as u64);
                } else {
                    report.capture_truncated = true;
                }

                if !cfg.progress_every.is_zero() {
                    let elapsed = now - start;
                    if elapsed >= next_progress {
                        observer.on_progress(&CaptureProgress {
                            elapsed,
                            datagrams: report.datagrams,
                            bytes: report.bytes,
                        });
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
    report.first_peer = first_peer.map(|p| p.to_string());
    report.arrival = ArrivalStats::of(capture.arrivals());
    Ok((capture, report))
}

/// The [`DatagramListener`] adapter: one bound socket, one datagram.
#[derive(Debug)]
pub struct UdpDatagramListener {
    socket: UdpSocket,
}

impl UdpDatagramListener {
    /// Binds [`BIND_ADDRESS`] on `port`.
    pub fn bind(port: u16) -> io::Result<Self> {
        Ok(UdpDatagramListener { socket: UdpSocket::bind((BIND_ADDRESS, port))? })
    }
}

impl DatagramListener for UdpDatagramListener {
    type Error = io::Error;

    fn local_address(&self) -> io::Result<String> {
        Ok(self.socket.local_addr()?.to_string())
    }

    fn receive_one(&self, buf: &mut [u8]) -> io::Result<(usize, String)> {
        let (n, from) = self.socket.recv_from(buf)?;
        Ok((n, from.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::application::ports::SilentObserver;
    use crate::domain::codec::encode;
    use crate::domain::fixtures::synthetic;
    use crate::domain::loss::compare::compare;

    /// A free port, learned by binding one and letting it go, so that parallel
    /// tests cannot collide on a fixed number.
    fn free_port() -> u16 {
        let probe = UdpSocket::bind((BIND_ADDRESS, 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        port
    }

    fn send_all(port: u16, msgs: Vec<crate::domain::message::ItchMessage>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
            // Give the receiver a moment to bind before the first datagram.
            std::thread::sleep(Duration::from_millis(150));
            let mut scratch = [0u8; MAX_MESSAGE_LEN];
            for m in &msgs {
                let n = encode(m, &mut scratch).unwrap();
                sock.send_to(&scratch[..n], ("127.0.0.1", port)).unwrap();
                std::thread::sleep(Duration::from_micros(200));
            }
        })
    }

    /// End to end over loopback, against a real socket.
    #[test]
    fn receives_a_real_stream_and_measures_it() {
        let port = free_port();
        let cfg = ReceiveConfig {
            port,
            startup_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_millis(300),
            expect: None,
            progress_every: Duration::ZERO,
            ..Default::default()
        };

        let msgs = synthetic(300);
        let sender = send_all(port, synthetic(300));

        let (capture, report) =
            UdpDatagramSource::new().capture(&cfg, &mut SilentObserver).unwrap();
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
        let c = compare(&msgs, &decoded);
        assert!(c.unexpected.is_empty(), "nothing foreign should appear: {:?}", c.unexpected);
        assert_eq!(c.matched, decoded.len() as u64);
    }

    /// The observer is the *only* way out of this loop, so the adapter must
    /// actually drive it — otherwise `listen` would run silently.
    #[test]
    fn the_capture_loop_narrates_itself_through_the_observer_alone() {
        #[derive(Default)]
        struct Recorder {
            listening: Vec<CaptureStart>,
            progress: Vec<CaptureProgress>,
        }
        impl CaptureObserver for Recorder {
            fn on_listening(&mut self, start: &CaptureStart) {
                self.listening.push(start.clone());
            }
            fn on_progress(&mut self, progress: &CaptureProgress) {
                self.progress.push(*progress);
            }
        }

        let port = free_port();
        let cfg = ReceiveConfig {
            port,
            startup_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_millis(300),
            progress_every: Duration::from_millis(10),
            ..Default::default()
        };

        let sender = send_all(port, synthetic(300));
        let mut recorder = Recorder::default();
        let (_, report) = UdpDatagramSource::new().capture(&cfg, &mut recorder).unwrap();
        sender.join().unwrap();

        assert_eq!(recorder.listening.len(), 1, "bound exactly once");
        let start = &recorder.listening[0];
        assert!(start.local.ends_with(&format!(":{port}")), "{}", start.local);
        assert_eq!(start.startup_timeout, cfg.startup_timeout);
        assert_eq!(start.idle_timeout, cfg.idle_timeout);

        assert!(!recorder.progress.is_empty(), "300 datagrams at 200 µs should tick the timer");
        let last = recorder.progress.last().unwrap();
        assert!(last.datagrams <= report.datagrams);
        assert!(last.bytes <= report.bytes);
    }

    /// Waiting for a transmitter that never starts is an error with a
    /// diagnosis, not a silent empty capture.
    #[test]
    fn a_stream_that_never_starts_is_an_error_that_says_why() {
        let cfg = ReceiveConfig {
            port: free_port(),
            startup_timeout: Duration::from_millis(150),
            progress_every: Duration::ZERO,
            ..Default::default()
        };
        let err = UdpDatagramSource::new().capture(&cfg, &mut SilentObserver).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(err.to_string().contains("no datagram arrived"), "{err}");
    }

    #[test]
    fn the_one_shot_listener_hands_back_the_datagram_and_its_sender() {
        let listener = UdpDatagramListener::bind(0).unwrap();
        let addr = listener.local_address().unwrap();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();
        assert!(port > 0);

        let sender = std::thread::spawn(move || {
            let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            sock.send_to(b"AAAA", ("127.0.0.1", port)).unwrap();
        });

        let mut buf = [0u8; RECV_BUF];
        let (n, from) = listener.receive_one(&mut buf).unwrap();
        sender.join().unwrap();
        assert_eq!(&buf[..n], b"AAAA");
        assert!(from.starts_with("127.0.0.1:"), "{from}");
    }
}
