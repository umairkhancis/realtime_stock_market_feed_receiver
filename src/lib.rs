//! Dependency-free (std only) ITCH 5.0 codec and UDP transmitter.
//!
//! Slice 1 (see `docs/1_SLICE.md`): one Add Order message, alone, as the entire
//! UDP payload. No envelope, no sequence numbers, no framing.

pub mod codec;
pub mod model;
pub mod formatter;

use std::env;
use std::net::UdpSocket;

use codec::{decode_add_order, timestamp_nanos};
use model::unpack_stock_symbol;
use formatter::{format_price, hex};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = env::args().nth(1).unwrap_or_else(|| "9000".into()).parse()?;

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
    let ts = timestamp_nanos(&msg);
    println!("\ndecoded ItchAddOrder:");
    println!("  message_type       {:?}", char::from(msg.message_type as u8));
    println!("  stock_locate       {}", { msg.stock_locate });
    println!("  tracking_number    {}", { msg.tracking_number });
    println!("  timestamp          {ts} ns since midnight ({:02}:{:02}:{:02})",
        ts / 3_600_000_000_000,
        (ts / 60_000_000_000) % 60,
        (ts / 1_000_000_000) % 60);
    println!("  order_reference    {}", { msg.order_reference });
    println!("  buy_sell_indicator {:?}", char::from(msg.buy_sell_indicator as u8));
    println!("  shares             {}", { msg.shares });
    println!("  stock              {:?}", unpack_stock_symbol(&{ msg.stock }));
    println!("  price              {} (raw {})", format_price(msg.price), { msg.price });
    Ok(())
}
