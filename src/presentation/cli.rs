//! The controller: argv in, use cases out.
//!
//! This module and `main.rs` together are the **composition root** — the one
//! place allowed to know every layer at once, because choosing which adapter
//! satisfies which port is precisely its job. Note the shape of each handler:
//! construct the concrete adapters, call the use cases, render what comes back.
//! No business logic reaches this far out, and no rendering reaches any further
//! in.
//!
//! The operator defaults live here rather than in the layers they configure,
//! for the same reason: `data/feed.csv` is a deployment choice, not a fact about
//! feeds. The use cases receive a `FeedStore` that already knows where it reads,
//! and never see a path at all.
//!
//! `listen` is worth reading as one block. It is a sequence of port calls and
//! domain results with rendering between them, which is exactly what a
//! controller is — and it is why there is no `listen` use case wrapping the
//! whole thing: an observer with six methods that existed only to re-emit what
//! is printed here would be cardboard, not architecture.

use std::time::Duration;

use crate::application::Result;
use crate::application::ports::{DatagramListener, FeedStore, SymbolSource};
use crate::application::receive::{ReceiveConfig, capture_feed};
use crate::application::slice_one::receive_single;
use crate::application::verify::{dump_capture, verify_against, verify_stored};
use crate::domain::codec::{decode_add_order, timestamp_nanos};
use crate::domain::message::unpack_stock_symbol;
use crate::domain::symbols::SymbolMap;
use crate::infrastructure::csv::{CsvFeedStore, CsvSymbolSource};
use crate::infrastructure::net::DEFAULT_PORT;
use crate::infrastructure::net::udp::{UdpDatagramListener, UdpDatagramSource};
use crate::presentation::console::{ConsoleObserver, print_decode_failures, print_report};
use crate::presentation::format::{format_price, hex};
use crate::presentation::report::{print_comparison, print_detection};
use crate::presentation::summary::summarise;

/// The transmitter's feed, and therefore this receiver's answer key.
pub const DEFAULT_TRUTH_CSV: &str = "data/feed.csv";
/// Where `listen` writes what arrived.
pub const DEFAULT_DUMP_CSV: &str = "data/received.csv";
/// The locate → ticker map the transmitter writes beside its feed.
pub const DEFAULT_SYMBOLS_CSV: &str = "data/feed.symbols.csv";

pub const USAGE: &str = "\
usage: rx <command> [options]

  listen    capture a feed and verify it
              --port N          (default 9000)
              --expect N        stop after N datagrams (e.g. 100000)
              --wait SECS       how long to wait for the first datagram (default 60)
              --idle-ms N       silence that ends the capture (default 2000)
              --csv PATH        the transmitter's feed.csv — enables exact verification
              --symbols PATH    the locate map (default: <csv stem>.symbols.csv, else learned)
              --dump PATH       write what arrived, in the transmitter's CSV format
              --focus TICKER    symbol for the volatility column
              --quiet           skip the summary tables

  summary   describe a captured or generated feed
              --csv PATH        required
              --symbols PATH / --focus TICKER

  verify    diff two CSVs offline, without touching the network
              --csv PATH        what was sent (ground truth)
              --received PATH   what arrived (from `listen --dump`)

  one       slice 1: receive a single Add Order and print its fields
              [PORT]            (default 9000)

exit status: 0 = verified, 1 = messages missing, 2 = error.

Typical run, receiver first:
  rx listen --expect 100000 --csv feed.csv --dump received.csv
  tx send --csv data/feed.csv --dest <this-host>:9000 --rate 10000";

/// Dispatches one command. Takes argv as a slice rather than reading the
/// environment so that a test can drive it.
pub fn run(args: &[String]) -> Result {
    let command = args.first().map(|s| s.as_str()).unwrap_or("help");

    match command {
        "listen" => listen(),
        "summary" => summary(),
        "verify" => verify(),
        "one" => listen_one(),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    }
}

/// Capture a feed, say what it lost, check it against ground truth, dump it.
fn listen() -> Result {
    let config = ReceiveConfig {
        port: DEFAULT_PORT,
        startup_timeout: Duration::from_secs(60u64),
        idle_timeout: Duration::from_millis(2_000u64),
        expect: None,
        ..Default::default()
    };
    let mut source = UdpDatagramSource::new();
    let mut observer = ConsoleObserver;

    let outcome = capture_feed(&mut source, &config, &mut observer)?;
    print_report(&outcome.report);
    print_decode_failures(&outcome.failures, outcome.report.datagrams);

    // Inference first — it is what a real deployment would have.
    print_detection(&outcome.detection);

    let truth = CsvFeedStore::new(DEFAULT_TRUTH_CSV);
    let verification = verify_against(&truth, &outcome.messages)?;
    println!();
    println!(
        "read {} expected messages from {}",
        verification.comparison.expected, verification.location
    );
    print_comparison(&verification.comparison);
    if !verification.comparison.is_perfect() {
        return Err(format!("verification failed").into());
    }

    let dump = CsvFeedStore::new(DEFAULT_DUMP_CSV);
    let stored = dump_capture(&dump, &outcome.messages)?;
    println!();
    println!("wrote {} received messages to {}", stored.rows, stored.location);
    println!("  (same 14 columns as the transmitter's feed.csv — with zero loss the two");
    println!("   files are byte-identical, so `diff` is a complete check on its own)");

    Ok(())
}

/// Describe a capture. A load and a render, so the composition root calls the
/// port directly rather than through a use case that would add nothing.
fn summary() -> Result {
    let store = CsvFeedStore::new(DEFAULT_DUMP_CSV);
    let messages = store.load()?;
    println!("read {} messages from {}", messages.len(), store.location());
    let symbols = load_symbols();
    summarise(&messages, &symbols, None);
    Ok(())
}

/// Diff two CSVs offline, without touching the network.
fn verify() -> Result {
    let truth = CsvFeedStore::new(DEFAULT_TRUTH_CSV);
    let capture = CsvFeedStore::new(DEFAULT_DUMP_CSV);

    let verification = verify_stored(&truth, &capture)?;
    println!(
        "expected {} messages from {}",
        verification.comparison.expected, verification.location
    );
    println!(
        "received {} messages from {}",
        verification.comparison.received,
        capture.location()
    );

    print_comparison(&verification.comparison);
    return Err(format!("verification failed").into());
}

/// Slice 1, unchanged in behaviour: one Add Order, printed field by field.
fn listen_one() -> Result {
    let listener = UdpDatagramListener::bind(DEFAULT_PORT)?;
    println!("listening on {}", listener.local_address()?);

    let one = receive_single(&listener)?;
    if one.truncated {
        eprintln!("warning: datagram filled the buffer — it may have been truncated by the kernel");
    }
    println!(
        "received {} bytes from {}:\n{}",
        one.payload.len(),
        one.from,
        hex(&one.payload)
    );

    let msg = decode_add_order(&one.payload)?;
    let ts = timestamp_nanos(&msg);
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

/// Resolves the locate → ticker map from the file the transmitter writes
/// beside its feed.
///
/// The `unwrap` is carried over verbatim from the pre-refactor code, and it is
/// a real wart: `summary` panics rather than erroring when the map is absent,
/// even though [`SymbolMap::learn`] could recover the names from the add stream.
/// Fixing it means changing behaviour, which this pass deliberately does not do
/// — `docs/clean_arch.md` lists it under what was left alone.
fn load_symbols() -> SymbolMap {
    let source = CsvSymbolSource::new(DEFAULT_SYMBOLS_CSV);
    let symbols = source.load().unwrap();
    println!("read {} symbols from {}", symbols.len(), source.location());
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn help_is_the_default_and_an_unknown_command_is_an_error() {
        assert!(run(&argv(&[])).is_ok());
        assert!(run(&argv(&["help"])).is_ok());
        assert!(run(&argv(&["-h"])).is_ok());
        assert!(run(&argv(&["--help"])).is_ok());

        let err = run(&argv(&["nonsense"])).unwrap_err().to_string();
        assert!(err.contains("unknown command"), "{err}");
        assert!(err.contains("usage: rx"), "usage must follow the error");
    }

    /// The defaults the usage text promises are the defaults the code uses.
    #[test]
    fn documented_defaults_are_the_real_ones() {
        let cfg = ReceiveConfig::default();
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(DEFAULT_PORT, 9000);
        assert_eq!(cfg.startup_timeout, Duration::from_secs(60));
        assert_eq!(cfg.idle_timeout, Duration::from_millis(2_000));

        for expected in ["9000", "60", "2000", "feed.csv", "received.csv"] {
            assert!(USAGE.contains(expected), "usage does not mention {expected}");
        }
    }

    /// The paths this command line hands the adapters, and the relationship
    /// between them the transmitter also relies on.
    #[test]
    fn the_symbol_map_is_the_one_beside_the_ground_truth() {
        assert_eq!(DEFAULT_TRUTH_CSV, "data/feed.csv");
        assert_eq!(DEFAULT_DUMP_CSV, "data/received.csv");
        assert_eq!(
            CsvSymbolSource::beside(CsvFeedStore::new(DEFAULT_TRUTH_CSV).path()).location(),
            DEFAULT_SYMBOLS_CSV
        );
    }
}
