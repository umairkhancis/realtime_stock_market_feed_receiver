//! The CSV side of the receiver.
//!
//! Two jobs, both against the transmitter's format exactly:
//!
//! - **Read** `feed.csv` — the transmitter's ground truth — so what arrived can
//!   be compared against what was sent, message by message. See [`crate::application::compare`].
//! - **Write** what actually arrived, in the *same* 14 columns and the same
//!   order, so that with zero loss the two files are byte-identical and plain
//!   `diff` is a complete verification.
//!
//! The `seq` column is not a wire field — nothing in the datagram carries a
//! sequence number in slice 2. On the transmitter it is the row's index in the
//! file; here it is the datagram's index in arrival order. With no loss those
//! agree, which is the whole point; with loss they diverge from the first gap
//! onward, which is why [`crate::application::compare`] exists rather than relying on `diff`.
//!
//! Only 'A' and 'F' carry an ASCII ticker, so the `stock` column is empty for
//! every other type — exactly as on the wire. [`read_symbol_table`] loads the
//! locate → ticker map the transmitter writes alongside the feed.

use std::fmt;
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use crate::domain::model::{
    pack_itch_timestamp, unpack_stock_symbol, ItchAddOrder, ItchAddOrderAttributed, ItchMessage,
    ItchOrderCancel, ItchOrderDelete, ItchOrderExecuted, ItchOrderExecutedWithPrice,
    ItchOrderReplace,
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
/// [`SymbolMap::learn`] does. Real ITCH has the same shape: you build the map
/// from the Stock Directory messages before the tape means anything.
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

/// The locate → ticker mapping, however the receiver came by it.
#[derive(Debug, Clone, Default)]
pub struct SymbolMap {
    by_locate: BTreeMap<u16, String>,
}

impl SymbolMap {
    pub fn new(by_locate: BTreeMap<u16, String>) -> Self {
        SymbolMap { by_locate }
    }

    /// Builds the map from the feed itself, out of the 'A' and 'F' messages.
    ///
    /// This is the mapping a receiver has without any out-of-band file, and it
    /// has a real failure mode worth naming: a symbol that never gets an add
    /// during the capture stays anonymous, and if the *first* add for a symbol
    /// is the one that was lost, the map is built from the second one instead —
    /// which is fine here only because the transmitter never reuses a locate.
    pub fn learn(msgs: &[ItchMessage]) -> Self {
        let mut by_locate = BTreeMap::new();
        for m in msgs {
            let (locate, stock) = match m {
                ItchMessage::AddOrder(a) => (a.stock_locate, a.stock),
                ItchMessage::AddOrderAttributed(a) => (a.stock_locate, a.stock),
                _ => continue,
            };
            by_locate
                .entry(locate)
                .or_insert_with(|| unpack_stock_symbol(&stock).to_string());
        }
        SymbolMap { by_locate }
    }

    /// Falls back to naming the locate when the ticker is unknown, rather than
    /// inventing one or panicking.
    pub fn name(&self, locate: u16) -> String {
        self.by_locate
            .get(&locate)
            .cloned()
            .unwrap_or_else(|| format!("locate:{locate}"))
    }

    pub fn locate_of(&self, ticker: &str) -> Option<u16> {
        self.by_locate.iter().find(|(_, t)| *t == ticker).map(|(l, _)| *l)
    }

    pub fn is_empty(&self) -> bool {
        self.by_locate.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_locate.len()
    }
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
        crate::domain::model::pack_stock_symbol(v).ok_or_else(|| FeedError::BadField {
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
    use crate::fixtures::synthetic;

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

        let map = SymbolMap::new(map);
        assert_eq!(map.name(3), "NVDA");
        assert_eq!(map.locate_of("NVDA"), Some(3));
        // An unknown locate is named, not guessed at and not a panic.
        assert_eq!(map.name(99), "locate:99");
        assert_eq!(map.locate_of("TSLA"), None);
    }

    #[test]
    fn a_symbol_map_can_be_learned_from_the_adds_alone() {
        let msgs = synthetic(5_000);
        let map = SymbolMap::learn(&msgs);
        assert_eq!(map.len(), crate::fixtures::TICKERS.len());
        for (i, ticker) in crate::fixtures::TICKERS.iter().enumerate() {
            assert_eq!(map.name(i as u16 + 1), *ticker);
        }
        // Every message in the feed resolves to a name.
        for m in &msgs {
            assert!(!map.name(m.stock_locate()).starts_with("locate:"));
        }
    }

    #[test]
    fn an_empty_symbol_map_still_names_everything() {
        let map = SymbolMap::default();
        assert!(map.is_empty());
        assert_eq!(map.name(7), "locate:7");
        assert_eq!(read_symbol_table("".as_bytes()).unwrap().len(), 0);
    }
}
