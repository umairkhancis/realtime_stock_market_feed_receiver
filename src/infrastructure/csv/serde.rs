//! The CSV side of the receiver.
//!
//! Two jobs, both against the transmitter's format exactly:
//!
//! - **Read** `feed.csv` — the transmitter's ground truth — so what arrived can
//!   be compared against what was sent, message by message. See
//!   [`crate::domain::loss::compare`].
//! - **Write** what actually arrived, in the *same* 14 columns and the same
//!   order, so that with zero loss the two files are byte-identical and plain
//!   `diff` is a complete verification.
//!
//! The `seq` column is not a wire field — nothing in the datagram carries a
//! sequence number in slice 2. On the transmitter it is the row's index in the
//! file; here it is the datagram's index in arrival order. With no loss those
//! agree, which is the whole point; with loss they diverge from the first gap
//! onward, which is why [`crate::domain::loss::compare`] exists rather than
//! relying on `diff`.
//!
//! Only 'A' and 'F' carry an ASCII ticker, so the `stock` column is empty for
//! every other type — exactly as on the wire. [`read_symbol_table`] loads the
//! locate → ticker map the transmitter writes alongside the feed; what a
//! receiver then *does* with a locate map is
//! [`crate::domain::symbols::SymbolMap`], one ring in.
//!
//! Everything here is generic over `impl BufRead` / `impl Write` and never
//! names a path. [`super::store`] is the half that owns `File`.

use std::fmt;
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use crate::domain::message::{
    ItchAddOrder, ItchAddOrderAttributed, ItchMessage, ItchOrderCancel, ItchOrderDelete,
    ItchOrderExecuted, ItchOrderExecutedWithPrice, ItchOrderReplace, pack_itch_timestamp,
    unpack_stock_symbol,
};

/// The header row, and the authority on column order.
pub const HEADER: &str = "seq,timestamp_ns,msg_type,stock_locate,tracking_number,stock,\
order_ref,new_order_ref,side,shares,price,match_number,printable,attribution";

const COLUMNS: usize = 14;

#[derive(Debug)]
pub enum FeedError {
    Io(io::Error),
    /// A row did not have [`COLUMNS`] fields.
    WrongColumnCount { line: u64, expected: usize, got: usize },
    /// A field was missing, malformed, or out of range.
    BadField { line: u64, column: &'static str, value: String },
    /// The type byte in column 2 is not one we speak.
    UnknownMessageType { line: u64, value: String },
    /// The header row is absent or renamed — refuse rather than silently
    /// misreading a file whose columns moved.
    BadHeader { got: String },
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedError::Io(e) => write!(f, "{e}"),
            FeedError::WrongColumnCount { line, expected, got } => {
                write!(f, "line {line}: expected {expected} columns, got {got}")
            }
            FeedError::BadField { line, column, value } => {
                write!(f, "line {line}: bad value {value:?} in column {column}")
            }
            FeedError::UnknownMessageType { line, value } => {
                write!(f, "line {line}: unknown message type {value:?}")
            }
            FeedError::BadHeader { got } => {
                write!(f, "not a feed CSV: expected header\n  {HEADER}\ngot\n  {got}")
            }
        }
    }
}

impl std::error::Error for FeedError {}

impl From<io::Error> for FeedError {
    fn from(e: io::Error) -> Self {
        FeedError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Writes `msgs` as CSV, header included. Returns the number of rows written.
pub fn write_feed<W, I>(out: &mut W, msgs: I) -> Result<u64, FeedError>
where
    W: Write,
    I: IntoIterator<Item = ItchMessage>,
{
    writeln!(out, "{HEADER}")?;
    let mut rows = 0u64;
    let mut line = String::with_capacity(128);
    for msg in msgs {
        line.clear();
        render_row(&mut line, rows, &msg);
        out.write_all(line.as_bytes())?;
        out.write_all(b"\n")?;
        rows += 1;
    }
    Ok(rows)
}

/// Loads the locate → ticker map the transmitter writes as
/// `<feed>.symbols.csv`.
///
/// Optional. Every message carries a locate but only 'A' and 'F' carry the
/// ticker, so without this file the receiver can still name a symbol — it just
/// has to learn the mapping from the add stream, which is what
/// [`crate::domain::symbols::SymbolMap::learn`] does. Real ITCH has the same
/// shape: you build the map from the Stock Directory messages before the tape
/// means anything.
pub fn read_symbol_table<R: BufRead>(input: R) -> Result<BTreeMap<u16, String>, FeedError> {
    let mut map = BTreeMap::new();
    for (i, line) in input.lines().enumerate() {
        let line = line?;
        let line = line.trim_end_matches('\r');
        if i == 0 || line.is_empty() {
            continue; // header
        }
        let mut f = line.split(',');
        let (Some(locate), Some(ticker)) = (f.next(), f.next()) else {
            return Err(FeedError::WrongColumnCount {
                line: i as u64 + 1,
                expected: 3,
                got: line.split(',').count(),
            });
        };
        let locate = locate.parse::<u16>().map_err(|_| FeedError::BadField {
            line: i as u64 + 1,
            column: "stock_locate",
            value: locate.to_string(),
        })?;
        map.insert(locate, ticker.to_string());
    }
    Ok(map)
}

fn render_row(buf: &mut String, seq: u64, msg: &ItchMessage) {
    use fmt::Write as _;
    let ts = msg.timestamp_nanos();
    let t = msg.message_type() as char;
    let locate = msg.stock_locate();

    // seq,timestamp_ns,msg_type,stock_locate,tracking_number,...
    match msg {
        ItchMessage::AddOrder(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},{},{},,{},{},{},,,",
                { m.tracking_number },
                unpack_stock_symbol(&{ m.stock }),
                { m.order_reference },
                m.buy_sell_indicator as char,
                { m.shares },
                { m.price },
            );
        }
        ItchMessage::AddOrderAttributed(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},{},{},,{},{},{},,,{}",
                { m.tracking_number },
                unpack_stock_symbol(&{ m.stock }),
                { m.order_reference },
                m.buy_sell_indicator as char,
                { m.shares },
                { m.price },
                ascii4(&{ m.attribution }),
            );
        }
        ItchMessage::OrderExecuted(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},,{},,,{},,{},,",
                { m.tracking_number },
                { m.order_reference },
                { m.shares },
                { m.match_number },
            );
        }
        ItchMessage::OrderExecutedWithPrice(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},,{},,,{},{},{},{},",
                { m.tracking_number },
                { m.order_reference },
                { m.shares },
                { m.execution_price },
                { m.match_number },
                m.printable as char,
            );
        }
        ItchMessage::OrderCancel(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},,{},,,{},,,,",
                { m.tracking_number },
                { m.order_reference },
                { m.canceled_shares },
            );
        }
        ItchMessage::OrderDelete(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},,{},,,,,,,",
                { m.tracking_number },
                { m.order_reference },
            );
        }
        ItchMessage::OrderReplace(m) => {
            let _ = write!(
                buf,
                "{seq},{ts},{t},{locate},{},,{},{},,{},{},,,",
                { m.tracking_number },
                { m.original_order_reference },
                { m.new_order_reference },
                { m.shares },
                { m.price },
            );
        }
    }
}

/// Renders a 4-byte MPID, trimming trailing spaces.
fn ascii4(field: &[u8; 4]) -> &str {
    let end = field.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    std::str::from_utf8(&field[..end]).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Reads a whole feed CSV into memory.
///
/// Deliberately not streaming. The transmitter has to hold a 100 µs schedule;
/// pulling the next row off a disk read mid-loop would put page-cache misses
/// straight into the inter-packet gap. Read it all, encode it all, *then* start
/// the clock. At 100,000 messages this is a few megabytes.
pub fn read_feed<R: BufRead>(input: R) -> Result<Vec<ItchMessage>, FeedError> {
    let mut out = Vec::new();
    let mut lines = input.lines();

    match lines.next() {
        None => return Err(FeedError::BadHeader { got: String::from("<empty file>") }),
        Some(header) => {
            let header = header?;
            if header.trim_end_matches('\r') != HEADER {
                return Err(FeedError::BadHeader { got: header });
            }
        }
    }

    for (i, line) in lines.enumerate() {
        let line = line?;
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        // Line numbers are 1-based and include the header, so a parse error can
        // be opened at the right place in an editor.
        out.push(parse_row(line, i as u64 + 2)?);
    }
    Ok(out)
}

fn parse_row(line: &str, no: u64) -> Result<ItchMessage, FeedError> {
    let f: Vec<&str> = line.split(',').collect();
    if f.len() != COLUMNS {
        return Err(FeedError::WrongColumnCount { line: no, expected: COLUMNS, got: f.len() });
    }

    let num = |v: &str, column: &'static str| -> Result<u64, FeedError> {
        v.parse::<u64>().map_err(|_| FeedError::BadField {
            line: no,
            column,
            value: v.to_string(),
        })
    };
    let u32f = |v: &str, column: &'static str| -> Result<u32, FeedError> {
        num(v, column).and_then(|n| {
            u32::try_from(n).map_err(|_| FeedError::BadField {
                line: no,
                column,
                value: v.to_string(),
            })
        })
    };
    let u16f = |v: &str, column: &'static str| -> Result<u16, FeedError> {
        num(v, column).and_then(|n| {
            u16::try_from(n).map_err(|_| FeedError::BadField {
                line: no,
                column,
                value: v.to_string(),
            })
        })
    };
    let one_byte = |v: &str, column: &'static str| -> Result<u8, FeedError> {
        let b = v.as_bytes();
        if b.len() == 1 {
            Ok(b[0])
        } else {
            Err(FeedError::BadField { line: no, column, value: v.to_string() })
        }
    };
    let stock = |v: &str| -> Result<[u8; 8], FeedError> {
        crate::domain::message::pack_stock_symbol(v).ok_or_else(|| FeedError::BadField {
            line: no,
            column: "stock",
            value: v.to_string(),
        })
    };

    let timestamp_bytes = pack_itch_timestamp(num(f[1], "timestamp_ns")?).ok_or_else(|| {
        FeedError::BadField { line: no, column: "timestamp_ns", value: f[1].to_string() }
    })?;
    let stock_locate = u16f(f[3], "stock_locate")?;
    let tracking_number = u16f(f[4], "tracking_number")?;
    let message_type = one_byte(f[2], "msg_type")?;

    Ok(match message_type {
        b'A' => ItchMessage::AddOrder(ItchAddOrder {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            order_reference: num(f[6], "order_ref")?,
            buy_sell_indicator: one_byte(f[8], "side")?,
            shares: u32f(f[9], "shares")?,
            stock: stock(f[5])?,
            price: u32f(f[10], "price")?,
        }),
        b'F' => ItchMessage::AddOrderAttributed(ItchAddOrderAttributed {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            order_reference: num(f[6], "order_ref")?,
            buy_sell_indicator: one_byte(f[8], "side")?,
            shares: u32f(f[9], "shares")?,
            stock: stock(f[5])?,
            price: u32f(f[10], "price")?,
            attribution: mpid(f[13]).ok_or_else(|| FeedError::BadField {
                line: no,
                column: "attribution",
                value: f[13].to_string(),
            })?,
        }),
        b'E' => ItchMessage::OrderExecuted(ItchOrderExecuted {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            order_reference: num(f[6], "order_ref")?,
            shares: u32f(f[9], "shares")?,
            match_number: num(f[11], "match_number")?,
        }),
        b'C' => ItchMessage::OrderExecutedWithPrice(ItchOrderExecutedWithPrice {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            order_reference: num(f[6], "order_ref")?,
            shares: u32f(f[9], "shares")?,
            match_number: num(f[11], "match_number")?,
            printable: one_byte(f[12], "printable")?,
            execution_price: u32f(f[10], "price")?,
        }),
        b'X' => ItchMessage::OrderCancel(ItchOrderCancel {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            order_reference: num(f[6], "order_ref")?,
            canceled_shares: u32f(f[9], "shares")?,
        }),
        b'D' => ItchMessage::OrderDelete(ItchOrderDelete {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            order_reference: num(f[6], "order_ref")?,
        }),
        b'U' => ItchMessage::OrderReplace(ItchOrderReplace {
            message_type,
            stock_locate,
            tracking_number,
            timestamp_bytes,
            original_order_reference: num(f[6], "order_ref")?,
            new_order_reference: num(f[7], "new_order_ref")?,
            shares: u32f(f[9], "shares")?,
            price: u32f(f[10], "price")?,
        }),
        _ => return Err(FeedError::UnknownMessageType { line: no, value: f[2].to_string() }),
    })
}

fn mpid(v: &str) -> Option<[u8; 4]> {
    let b = v.as_bytes();
    if b.len() > 4 || !b.iter().all(|c| c.is_ascii_graphic()) {
        return None;
    }
    let mut out = [b' '; 4];
    out[..b.len()].copy_from_slice(b);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fixtures::{drop_every, synthetic};
    use crate::domain::loss::compare::compare;

    fn round_trip(count: u64) -> (Vec<ItchMessage>, Vec<ItchMessage>) {
        let original: Vec<ItchMessage> = synthetic(count as usize);
        let mut csv: Vec<u8> = Vec::new();
        let rows = write_feed(&mut csv, original.clone()).unwrap();
        assert_eq!(rows, count);
        let parsed = read_feed(csv.as_slice()).unwrap();
        (original, parsed)
    }

    /// The CSV is only ground truth if it is lossless. Every field of every
    /// variant has to survive the trip through text.
    #[test]
    fn csv_round_trips_every_message_exactly() {
        let (original, parsed) = round_trip(20_000);
        assert_eq!(parsed.len(), original.len());
        for (i, (a, b)) in original.iter().zip(parsed.iter()).enumerate() {
            assert_eq!(a, b, "row {i} did not survive the round trip");
        }
    }

    /// Which is the same as saying: the bytes on the wire are the same whether
    /// they came from the generator or from the file.
    #[test]
    fn csv_round_trip_preserves_the_encoded_bytes() {
        let (original, parsed) = round_trip(5_000);
        let mut a = [0u8; crate::domain::codec::MAX_MESSAGE_LEN];
        let mut b = [0u8; crate::domain::codec::MAX_MESSAGE_LEN];
        for (x, y) in original.iter().zip(parsed.iter()) {
            let na = crate::domain::codec::encode(x, &mut a).unwrap();
            let nb = crate::domain::codec::encode(y, &mut b).unwrap();
            assert_eq!(a[..na], b[..nb]);
        }
    }

    #[test]
    fn every_row_has_the_declared_columns() {
        let mut csv: Vec<u8> = Vec::new();
        write_feed(&mut csv, synthetic(3_000)).unwrap();
        let text = String::from_utf8(csv).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), HEADER);
        assert_eq!(HEADER.split(',').count(), COLUMNS);
        for (i, line) in lines.enumerate() {
            assert_eq!(
                line.split(',').count(),
                COLUMNS,
                "row {i} has the wrong column count: {line}"
            );
            assert!(!line.contains(",,,,,,,,,,,,,"), "row {i} is entirely empty: {line}");
        }
    }

    #[test]
    fn seq_column_counts_from_zero_without_gaps() {
        let mut csv: Vec<u8> = Vec::new();
        write_feed(&mut csv, synthetic(1_000)).unwrap();
        let text = String::from_utf8(csv).unwrap();
        for (i, line) in text.lines().skip(1).enumerate() {
            assert_eq!(line.split(',').next().unwrap(), i.to_string());
        }
    }

    #[test]
    fn rejects_a_file_whose_columns_moved() {
        let bad = "seq,timestamp_ns,msg_type\n0,1,A\n";
        assert!(matches!(read_feed(bad.as_bytes()), Err(FeedError::BadHeader { .. })));
        assert!(matches!(read_feed("".as_bytes()), Err(FeedError::BadHeader { .. })));
    }

    #[test]
    fn reports_the_line_number_of_a_bad_row() {
        let mut text = String::from(HEADER);
        text.push('\n');
        text.push_str("0,34200000000000,A,1,0,AAPL,9,,B,100,1500000,,,\n");
        text.push_str("1,34200000100000,A,1,0,AAPL,notanumber,,B,100,1500000,,,\n");
        match read_feed(text.as_bytes()) {
            Err(FeedError::BadField { line, column, .. }) => {
                assert_eq!(line, 3, "header is line 1, so the bad row is line 3");
                assert_eq!(column, "order_ref");
            }
            other => panic!("expected a BadField, got {other:?}"),
        }

        let mut short = String::from(HEADER);
        short.push_str("\n0,1,A,1,0\n");
        assert!(matches!(
            read_feed(short.as_bytes()),
            Err(FeedError::WrongColumnCount { line: 2, got: 5, .. })
        ));

        let mut unknown = String::from(HEADER);
        unknown.push_str("\n0,34200000000000,P,1,0,,9,,,,,,,\n");
        assert!(matches!(
            read_feed(unknown.as_bytes()),
            Err(FeedError::UnknownMessageType { line: 2, .. })
        ));
    }

    #[test]
    fn symbol_table_round_trips_from_the_transmitter_format() {
        let text = "stock_locate,ticker,open_price
1,AAPL,1500000
3,NVDA,1200000
";
        let map = read_symbol_table(text.as_bytes()).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&1], "AAPL");
        assert_eq!(map[&3], "NVDA");
        // The third column is the transmitter's opening price; the receiver
        // reads the map, not the market.
        assert_eq!(read_symbol_table("".as_bytes()).unwrap().len(), 0);
    }

    // -----------------------------------------------------------------------
    // Cross-layer: the CSV format and the ground-truth comparison together.
    //
    // These span `infrastructure::csv` and `domain::loss::compare`, so strictly
    // they belong in `tests/`, as the transmitter's do. They stay here because
    // building a known feed needs `domain::fixtures`, which is `#[cfg(test)]`
    // on purpose — a receiver that ships a feed generator is a receiver that
    // will eventually be tested against itself. Publishing the fixtures to
    // reach them from an integration test would cost more than the move buys.
    // -----------------------------------------------------------------------

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
