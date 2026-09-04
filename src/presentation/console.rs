//! The presenter: where every `println!` from the capture path ended up.
//!
//! [`ConsoleObserver`] is the [`CaptureObserver`] implementation the `listen`
//! command wires in. The capture loop pushes facts at it — bound, progress —
//! instead of formatting strings, which is what let `println!` leave
//! [`crate::infrastructure::net::udp`] entirely. That matters more here than it
//! would in most programs: printing inside the receive loop is a documented way
//! to make the terminal emulator the bottleneck and misread the resulting
//! socket-buffer overflow as network loss.

use crate::application::ports::{CaptureObserver, CaptureProgress, CaptureStart};
use crate::application::receive::ReceiveReport;
use crate::domain::codec::CodecError;

/// Renders a capture's progress to stdout.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsoleObserver;

impl CaptureObserver for ConsoleObserver {
    fn on_listening(&mut self, start: &CaptureStart) {
        println!("listening on {}", start.local);
        println!(
            "waiting up to {:?} for the first datagram, then ending after {:?} of silence",
            start.startup_timeout, start.idle_timeout
        );
    }

    fn on_progress(&mut self, p: &CaptureProgress) {
        println!(
            "  {:>5.1}s  {:>7} datagrams  {:>8.0}/s  {:>10} bytes",
            p.elapsed.as_secs_f64(),
            p.datagrams,
            p.datagrams as f64 / p.elapsed.as_secs_f64(),
            p.bytes,
        );
    }
}

/// What the transport saw, as opposed to what the messages say.
pub fn print_report(report: &ReceiveReport) {
    println!();
    println!("== transport ==");
    println!("  datagrams       {}", report.datagrams);
    println!(
        "  payload         {} bytes, {:.1} bytes/datagram average",
        report.bytes,
        report.bytes as f64 / report.datagrams.max(1) as f64
    );
    if let Some(p) = &report.first_peer {
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

/// Datagrams that arrived but did not decode.
///
/// Capped at ten lines: a run that corrupts 100,000 datagrams has one problem,
/// not 100,000, and a screenful of them is the same information as a count.
pub fn print_decode_failures(failures: &[(usize, CodecError)], datagrams: u64) {
    if failures.is_empty() {
        return;
    }
    println!();
    println!("== decode failures ==");
    for (i, e) in failures.iter().take(10) {
        println!("  datagram {i}: {e}");
    }
    if failures.len() > 10 {
        println!("  … {} more", failures.len() - 10);
    }
    println!(
        "  ({} of {} datagrams did not decode; one bad datagram costs exactly one",
        failures.len(),
        datagrams
    );
    println!("   message, because one datagram is one message)");
}
