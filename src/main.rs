//! The composition root's outermost shell.
//!
//! Deliberately almost empty. Everything worth testing lives in the library
//! crate beside it — the split the Rust Book's I/O project recommends, so that
//! `main` is left with only the two things a binary can do that a library
//! cannot: read the process environment, and set an exit code. In particular
//! [`cli::run`] takes `&[String]` rather than reading argv itself, which is what
//! makes its two tests possible at all.

use std::env;
use std::process;

use realtime_stock_market_feed_receiver::presentation::banner::dramatic_display;
use realtime_stock_market_feed_receiver::presentation::cli;

fn main() {
    dramatic_display("RT Receiver");

    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(e) = cli::run(&args) {
        eprintln!("RT Receiver encountered an error: {e}");
        process::exit(1);
    }
}
