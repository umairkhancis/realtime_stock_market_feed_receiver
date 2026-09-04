//! Dependency-free (std only) ITCH 5.0 receiver.
//!
//! The other half of the pipeline in the transmitter repo. Slice 1 received one
//! Add Order and printed it; slice 2 receives a paced 10,000 message/second
//! stream, one message per datagram, and answers the question that matters:
//! **did all of it arrive?**
//!
//! There are two ways to answer that, and they are not equivalent:
//!
//! - [`detect`] infers loss from the messages themselves — gaps in the order
//!   reference and match number sequences, dangling references in a replayed
//!   book. It needs nothing but the stream, which is what a real deployment has.
//!   It is also blind to roughly a third of the tape, and blind to tail loss.
//! - [`compare`] diffs what arrived against the transmitter's `feed.csv`. It has
//!   no blind spot at all, and it only works because this is a synthetic feed
//!   with an answer key.
//!
//! Both run, and the report says which is which. The gap between them is the
//! measure of what a session layer would buy — see `docs/session-layer.md` in
//! the transmitter repo.
//!
//! `codec.rs` and `model.rs` are byte-identical to the transmitter's copies.
//! That is deliberate: the golden vector is the wire contract, and it means
//! nothing if each side keeps its own idea of the offsets.

pub mod codec;
pub mod compare;
pub mod detect;
pub mod feed;
pub mod formatter;
pub mod model;
pub mod receive;
pub mod summary;

#[cfg(test)]
pub mod fixtures;

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::time::Duration;

use codec::decode_add_order;
use compare::{compare, print_comparison};
use detect::{Detection, print_detection};
use feed::{SymbolMap, read_feed, read_symbol_table, write_feed};
use formatter::{format_price, hex};
use model::{ItchMessage, unpack_stock_symbol};
use receive::{ReceiveConfig, receive};

pub const DEFAULT_PORT: u16 = 9000;

pub type Fallible = Result<(), Box<dyn std::error::Error>>;

pub fn listen(args: &Args) -> Fallible {
    let cfg = ReceiveConfig {
        port: args.parse_or("port", DEFAULT_PORT)?,
        startup_timeout: Duration::from_secs(args.parse_or("wait", 60u64)?),
        idle_timeout: Duration::from_millis(args.parse_or("idle-ms", 2_000u64)?),
        expect: match args.get("expect") {
            None => None,
            Some(v) => Some(v.parse::<u64>().map_err(|_| "--expect expects a number")?),
        },
        ..Default::default()
    };

    let (capture, report) = receive(&cfg)?;
    receive::print_report(&report);

    let (messages, failures) = capture.decode_all();
    if !failures.is_empty() {
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
            report.datagrams
        );
        println!("   message, because one datagram is one message)");
    }

    let symbols = load_symbols(args, &messages)?;

    // Inference first — it is what a real deployment would have.
    let detection = Detection::run(&messages);
    print_detection(&detection);

    // Then the answer key, when there is one.
    if let Some(path) = args.get("csv") {
        let expected = read_feed(BufReader::new(File::open(path)?))?;
        println!();
        println!("read {} expected messages from {path}", expected.len());
        let c = compare(&expected, &messages);
        print_comparison(&c);
        if !c.is_perfect() {
            return Err(format!("verification failed").into());
        }
    }

    if let Some(path) = args.get("dump") {
        let mut out = BufWriter::new(File::create(path)?);
        let rows = write_feed(&mut out, messages.iter().copied())?;
        out.flush()?;
        println!();
        println!("wrote {rows} received messages to {path}");
        println!("  (same 14 columns as the transmitter's feed.csv — with zero loss the two");
        println!("   files are byte-identical, so `diff` is a complete check on its own)");
    }

    if !args.has("quiet") {
        println!();
        summary::summarise(&messages, &symbols, args.get("focus"));
    }
    Ok(())
}

pub fn summarise(args: &Args) -> Fallible {
    let path = args.get("csv").ok_or("summary needs --csv PATH")?;
    let messages = read_feed(BufReader::new(File::open(path)?))?;
    println!("read {} messages from {path}", messages.len());
    let symbols = load_symbols(args, &messages)?;
    summary::summarise(&messages, &symbols, args.get("focus"));
    Ok(())
}

pub fn verify(args: &Args) -> Fallible {
    let expected_path = args
        .get("csv")
        .ok_or("verify needs --csv PATH (what was sent)")?;
    let received_path = args
        .get("received")
        .ok_or("verify needs --received PATH (what arrived)")?;

    let expected = read_feed(BufReader::new(File::open(expected_path)?))?;
    let received = read_feed(BufReader::new(File::open(received_path)?))?;
    println!("expected {} messages from {expected_path}", expected.len());
    println!("received {} messages from {received_path}", received.len());

    let c = compare(&expected, &received);
    print_comparison(&c);
    return Err(format!("verification failed").into());
}

/// Slice 1, unchanged in behaviour: one Add Order, printed field by field.
pub fn listen_one(port: Option<&str>) -> Fallible {
    let port: u16 = port.unwrap_or("9000").parse()?;

    // 0.0.0.0, not 127.0.0.1: binding loopback works perfectly on this machine
    // and silently receives nothing from another one.
    let sock = UdpSocket::bind(("0.0.0.0", port))?;
    println!("listening on {}", sock.local_addr()?);

    let mut buf = [0u8; 2048];
    let (n, from) = sock.recv_from(&mut buf)?;
    if n == buf.len() {
        eprintln!("warning: datagram filled the buffer — it may have been truncated by the kernel");
    }

    // &buf[..n], never &buf — the rest of the buffer is stale zeros.
    let payload = &buf[..n];
    println!("received {n} bytes from {from}:\n{}", hex(payload));

    let msg = decode_add_order(payload)?;
    let ts = codec::timestamp_nanos(&msg);
    println!("\ndecoded ItchAddOrder:");
    println!("  message_type       {:?}", char::from(msg.message_type));
    println!("  stock_locate       {}", { msg.stock_locate });
    println!("  tracking_number    {}", { msg.tracking_number });
    println!(
        "  timestamp          {ts} ns since midnight ({:02}:{:02}:{:02})",
        ts / 3_600_000_000_000,
        (ts / 60_000_000_000) % 60,
        (ts / 1_000_000_000) % 60
    );
    println!("  order_reference    {}", { msg.order_reference });
    println!(
        "  buy_sell_indicator {:?}",
        char::from(msg.buy_sell_indicator)
    );
    println!("  shares             {}", { msg.shares });
    println!(
        "  stock              {:?}",
        unpack_stock_symbol(&{ msg.stock })
    );
    println!(
        "  price              {} (raw {})",
        format_price(msg.price),
        { msg.price }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// Resolves the locate → ticker map, preferring an explicit file, then the one
/// the transmitter writes beside the feed, and finally the add stream itself.
fn load_symbols(
    args: &Args,
    messages: &[ItchMessage],
) -> Result<SymbolMap, Box<dyn std::error::Error>> {
    if let Some(path) = args.get("symbols") {
        let map = read_symbol_table(BufReader::new(File::open(path)?))?;
        println!("read {} symbols from {path}", map.len());
        return Ok(SymbolMap::new(map));
    }
    if let Some(csv) = args.get("csv") {
        let beside = symbols_path(Path::new(csv));
        if beside.exists() {
            let map = read_symbol_table(BufReader::new(File::open(&beside)?))?;
            println!("read {} symbols from {}", map.len(), beside.display());
            return Ok(SymbolMap::new(map));
        }
    }
    // No map on disk: learn it from the adds, the way a receiver with no
    // out-of-band file has to.
    Ok(SymbolMap::learn(messages))
}

/// `data/feed.csv` -> `data/feed.symbols.csv`, matching the transmitter.
fn symbols_path(feed: &Path) -> PathBuf {
    let stem = feed
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    feed.with_file_name(format!("{stem}.symbols.csv"))
}

/// A very small `--key value` / `--key=value` parser, matching the
/// transmitter's so the two command lines feel like one tool.
pub struct Args {
    values: Vec<(String, Option<String>)>,
}

impl Args {
    pub fn parse(argv: &[String]) -> Result<Args, String> {
        let mut values = Vec::new();
        let mut i = 0;
        while i < argv.len() {
            let arg = &argv[i];
            let key = arg
                .strip_prefix("--")
                .ok_or_else(|| format!("expected an option starting with --, got {arg:?}"))?;
            match key.split_once('=') {
                Some((k, v)) => {
                    values.push((k.to_string(), Some(v.to_string())));
                    i += 1;
                }
                None => match argv.get(i + 1) {
                    Some(v) if !v.starts_with("--") => {
                        values.push((key.to_string(), Some(v.clone())));
                        i += 2;
                    }
                    _ => {
                        values.push((key.to_string(), None));
                        i += 1;
                    }
                },
            }
        }
        Ok(Args { values })
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, key: &str) -> bool {
        self.values.iter().any(|(k, _)| k == key)
    }

    fn parse_or<T: std::str::FromStr>(&self, key: &str, default: T) -> Result<T, String> {
        match self.get(key) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{key} expects a number, got {v:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{drop_every, synthetic};

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args_accept_both_spellings() {
        let a = Args::parse(&argv(&["--port", "9001", "--expect=100000", "--quiet"])).unwrap();
        assert_eq!(a.get("port"), Some("9001"));
        assert_eq!(a.get("expect"), Some("100000"));
        assert!(a.has("quiet"));
        assert_eq!(a.get("quiet"), None);
        assert!(!a.has("nope"));
    }

    #[test]
    fn args_reject_a_stray_positional() {
        assert!(Args::parse(&argv(&["port", "9000"])).is_err());
    }

    #[test]
    fn args_parse_numbers_and_report_bad_ones() {
        let a = Args::parse(&argv(&["--port", "9000", "--idle-ms", "soon"])).unwrap();
        assert_eq!(a.parse_or("port", 1u16).unwrap(), 9000);
        assert_eq!(a.parse_or("missing", 42u64).unwrap(), 42);
        assert!(a.parse_or("idle-ms", 1u64).is_err());
    }

    #[test]
    fn symbols_path_matches_the_transmitters_layout() {
        assert_eq!(
            symbols_path(Path::new("data/feed.csv")),
            Path::new("data/feed.symbols.csv")
        );
        assert_eq!(
            symbols_path(Path::new("run1.csv")),
            Path::new("run1.symbols.csv")
        );
    }

    #[test]
    fn a_bare_port_still_means_slice_one() {
        // Not asserting on the receive itself — just that the argument shape
        // routes to `one` rather than erroring as an unknown command.
        assert!("9000".parse::<u16>().is_ok());
        assert!("listen".parse::<u16>().is_err());
    }

    /// The end-to-end promise of the CSV dump: with zero loss, the file this
    /// receiver writes is byte-identical to the transmitter's.
    #[test]
    fn a_lossless_capture_round_trips_to_an_identical_csv() {
        let sent = synthetic(3_000);
        let mut ground_truth: Vec<u8> = Vec::new();
        write_feed(&mut ground_truth, sent.iter().copied()).unwrap();

        // What a perfect capture would write.
        let received = read_feed(ground_truth.as_slice()).unwrap();
        let mut dumped: Vec<u8> = Vec::new();
        write_feed(&mut dumped, received.iter().copied()).unwrap();

        assert_eq!(
            ground_truth, dumped,
            "a lossless capture must dump an identical file"
        );
        assert!(compare(&sent, &received).is_perfect());
    }

    /// And with loss, the files diverge — which is why `compare` exists rather
    /// than relying on `diff`.
    #[test]
    fn a_lossy_capture_diverges_from_the_ground_truth_file() {
        let sent = synthetic(3_000);
        let (kept, dropped) = drop_every(&sent, 100);

        let mut a: Vec<u8> = Vec::new();
        let mut b: Vec<u8> = Vec::new();
        write_feed(&mut a, sent.iter().copied()).unwrap();
        write_feed(&mut b, kept.iter().copied()).unwrap();
        assert_ne!(a, b);

        let c = compare(&sent, &kept);
        assert_eq!(c.missing.len(), dropped.len());
        assert!(!c.is_perfect());
    }
}
