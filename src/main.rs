use realtime_stock_market_feed_receiver::formatter::dramatic_display;
use realtime_stock_market_feed_receiver::run;
use std::process;

fn main() {
    dramatic_display("RT Feed Receiver");
    // The exit status is part of the interface: 0 verified, 1 messages missing,
    // 2 the receiver itself failed. Without the split, a script cannot tell
    // "the feed was lossy" from "the receiver could not bind the port".
    match run() {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("RT Feed Receiver encountered an error: {e}");
            process::exit(2);
        }
    }
}
