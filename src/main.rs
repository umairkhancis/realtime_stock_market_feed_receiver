use realtime_stock_market_feed_receiver::presentation::formatter::dramatic_display;
use realtime_stock_market_feed_receiver::{Fallible, listen, listen_one, summarise, verify};
use std::env;
use std::process;

fn main() {
    dramatic_display("RT Receiver");
    if let Err(e) = run() {
        eprintln!("RT Receiver encountered an error: {e}");
        process::exit(1);
    }
}

const USAGE: &str = "\
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

pub fn run() -> Fallible {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.get(0).map(|s| s.as_str()).unwrap_or("help");

    match command {
        "listen" => listen(),
        "summary" => summarise(),
        "verify" => verify(),
        "one" => listen_one(),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    }
}
