use realtime_stock_market_feed_receiver::formatter::dramatic_display;
use realtime_stock_market_feed_receiver::run;
use std::process;

fn main() {
    dramatic_display("RT Feed Receiver");
    if let Err(e) = run() {
        eprintln!("RT Feed Receiver ecountered an error: {e}");
        process::exit(1);
    }
}
