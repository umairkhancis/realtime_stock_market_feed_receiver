//! Describes a received feed — the same three tables the transmitter prints,
//! computed from what actually arrived.
//!
//! This is the comparison that matters once the counts line up. Two runs of the
//! same feed should produce the same message mix, the same per-symbol shares,
//! and the same session shape: an opening burst, a calm middle, a sharp shock
//! just past halfway, a ramp into the close. If the receiver's copy of that
//! table has a different shape, it did not get all the messages — and unlike a
//! raw count, the *shape* says roughly where.
//!
//! This module still interleaves computation with `println!`, and that is a
//! deliberate hold rather than an oversight. Its pure half — `focus_mids`,
//! `realized_vol`, `choose_focus` — is already separable and already tested;
//! splitting it into a `domain::analytics` that returns a struct and a presenter
//! that renders it is the honest next step, and it was skipped because it is a
//! real implementation change rather than a structural one. The transmitter's
//! `docs/clean_arch.md` records the identical decision about its own `summary`,
//! and the two should move together when either does.
//!
//! One deliberate difference from the transmitter's version: nothing here knows
//! the symbol universe. The transmitter has a static table of eight tickers; the
//! receiver has whatever it saw. Locates come from every message, tickers only
//! from 'A' and 'F', and the focus symbol is chosen from the data rather than
//! hardcoded. A receiver that assumed the transmitter's table would be lying
//! about what it actually observed.

use std::collections::BTreeMap;

use crate::domain::message::ItchMessage;
use crate::domain::symbols::SymbolMap;
use crate::presentation::format::format_price;

/// Timeline granularity, in message-timestamp nanoseconds.
const BUCKET_NANOS: u64 = 1_000_000_000;

/// Sub-bucket for the realized-volatility estimate: ten samples per row.
const SUB_BUCKET_NANOS: u64 = 100_000_000;

pub fn summarise(msgs: &[ItchMessage], symbols: &SymbolMap, focus: Option<&str>) {
    if msgs.is_empty() {
        println!("empty feed — nothing to summarise");
        return;
    }

    let first_ts = msgs[0].timestamp_nanos();
    let last_ts = msgs[msgs.len() - 1].timestamp_nanos();
    let span_nanos = last_ts.saturating_sub(first_ts);
    let span_secs = span_nanos as f64 / 1e9;
    let wire_bytes: u64 = msgs.iter().map(|m| m.wire_len() as u64).sum();

    println!("== feed ==");
    println!("  messages        {}", msgs.len());
    println!(
        "  session         {} -> {}  ({:.3}s of market time)",
        clock(first_ts),
        clock(last_ts),
        span_secs
    );
    println!(
        "  wire            {} bytes, {:.1} bytes/message average",
        wire_bytes,
        wire_bytes as f64 / msgs.len() as f64
    );
    if span_secs > 0.0 {
        println!(
            "  implied rate    {:.0} msg/s = {:.0} packets/s at 1:1, {:.2} Mbps of ITCH payload",
            msgs.len() as f64 / span_secs,
            msgs.len() as f64 / span_secs,
            wire_bytes as f64 * 8.0 / span_secs / 1e6,
        );
    }

    print_message_mix(msgs);
    print_symbols(msgs, symbols);
    print_timeline(msgs, symbols, first_ts, focus);
}

fn clock(nanos: u64) -> String {
    let secs = nanos / 1_000_000_000;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}.{:09}", nanos % 1_000_000_000)
}

fn label(t: u8) -> &'static str {
    match t {
        b'A' => "A  add order",
        b'F' => "F  add order, attributed",
        b'E' => "E  order executed",
        b'C' => "C  order executed, priced",
        b'X' => "X  order cancel (partial)",
        b'D' => "D  order delete",
        b'U' => "U  order replace",
        _ => "?  unknown",
    }
}

fn print_message_mix(msgs: &[ItchMessage]) {
    println!();
    println!("== message mix ==");
    let types = [b'A', b'F', b'E', b'C', b'X', b'D', b'U'];
    let mut counts = [0u64; 7];
    for m in msgs {
        if let Some(i) = types.iter().position(|&t| t == m.message_type()) {
            counts[i] += 1;
        }
    }
    let total = msgs.len() as f64;
    let peak = counts.iter().copied().max().unwrap_or(1).max(1);
    for (i, &t) in types.iter().enumerate() {
        let share = counts[i] as f64 / total;
        let bar =
            "#".repeat(((counts[i] * 40 / peak) as usize).max(if counts[i] > 0 { 1 } else { 0 }));
        println!("  {:<28} {:>7}  {:>5.2}%  {bar}", label(t), counts[i], share * 100.0);
    }
}

/// Per-symbol activity and the price range the adds imply. Only 'A' and 'F'
/// carry a price and a ticker, so the price columns are built from those alone.
fn print_symbols(msgs: &[ItchMessage], symbols: &SymbolMap) {
    println!();
    println!("== symbols ==");
    println!(
        "  {:<10} {:>8} {:>7} {:>7} {:>12} {:>12} {:>12} {:>12}",
        "ticker", "msgs", "share", "adds", "open", "last", "low", "high"
    );

    #[derive(Default)]
    struct Row {
        messages: u64,
        adds: u64,
        first: Option<u32>,
        last: u32,
        lo: u32,
        hi: u32,
    }

    let mut rows: BTreeMap<u16, Row> = BTreeMap::new();
    for m in msgs {
        let row = rows.entry(m.stock_locate()).or_insert_with(|| Row { lo: u32::MAX, ..Default::default() });
        row.messages += 1;
        if let Some(p) = add_price(m) {
            row.adds += 1;
            row.first.get_or_insert(p);
            row.last = p;
            row.lo = row.lo.min(p);
            row.hi = row.hi.max(p);
        }
    }

    for (locate, row) in &rows {
        let share = row.messages as f64 / msgs.len() as f64 * 100.0;
        let name = symbols.name(*locate);
        match row.first {
            Some(f) => println!(
                "  {name:<10} {:>8} {share:>6.2}% {:>7} {:>12} {:>12} {:>12} {:>12}",
                row.messages,
                row.adds,
                format_price(f),
                format_price(row.last),
                format_price(row.lo),
                format_price(row.hi),
            ),
            // A symbol seen only through deletes and executions: it has a
            // locate and a message count, and no price and no name.
            None => println!(
                "  {name:<10} {:>8} {share:>6.2}% {:>7} {:>12}",
                row.messages, row.adds, "-"
            ),
        }
    }
    println!("  (prices are resting-order prices, not trades — the book's edges, not its mid)");
}

fn add_price(m: &ItchMessage) -> Option<u32> {
    add_side_price(m).map(|(_, p)| p)
}

fn add_side_price(m: &ItchMessage) -> Option<(u8, u32)> {
    match m {
        ItchMessage::AddOrder(a) => Some((a.buy_sell_indicator, a.price)),
        ItchMessage::AddOrderAttributed(a) => Some((a.buy_sell_indicator, a.price)),
        _ => None,
    }
}

/// Picks the symbol the timeline reports on: the named one if it was seen, else
/// whichever locate carried the most adds.
fn choose_focus(msgs: &[ItchMessage], symbols: &SymbolMap, requested: Option<&str>) -> Option<u16> {
    if let Some(ticker) = requested {
        if let Some(locate) = symbols.locate_of(ticker) {
            return Some(locate);
        }
        println!("  (no symbol named {ticker:?} in this capture; falling back to the busiest)");
    }
    let mut adds: BTreeMap<u16, u64> = BTreeMap::new();
    for m in msgs {
        if add_price(m).is_some() {
            *adds.entry(m.stock_locate()).or_default() += 1;
        }
    }
    adds.into_iter().max_by_key(|&(_, n)| n).map(|(l, _)| l)
}

/// Estimates the focus symbol's mid price in each [`SUB_BUCKET_NANOS`] window,
/// from the touch: the highest bid and lowest ask added inside it.
///
/// The obvious estimator — the mean of every add price in the window — is much
/// worse, and worth understanding why. Adds rest at a range of depths, so their
/// mean carries the depth distribution's dispersion as noise, and over 100 ms
/// that noise is the same size as the price move being measured. The touch is
/// pinned to the mid by construction, so it barely has any.
///
/// `None` marks a window with no two-sided quote — nothing to measure.
fn focus_mids(msgs: &[ItchMessage], locate: u16, first_ts: u64, n_sub: usize) -> Vec<Option<f64>> {
    let mut best_bid = vec![0u32; n_sub];
    let mut best_ask = vec![u32::MAX; n_sub];
    for m in msgs {
        if m.stock_locate() != locate {
            continue;
        }
        let Some((side, price)) = add_side_price(m) else { continue };
        let s = ((m.timestamp_nanos() - first_ts) / SUB_BUCKET_NANOS) as usize;
        if s >= n_sub {
            continue;
        }
        if side == b'B' {
            best_bid[s] = best_bid[s].max(price);
        } else {
            best_ask[s] = best_ask[s].min(price);
        }
    }
    (0..n_sub)
        .map(|s| {
            if best_bid[s] > 0 && best_ask[s] < u32::MAX {
                Some((best_bid[s] as f64 + best_ask[s] as f64) / 2.0)
            } else {
                None
            }
        })
        .collect()
}

fn print_timeline(msgs: &[ItchMessage], symbols: &SymbolMap, first_ts: u64, focus: Option<&str>) {
    println!();
    println!("== timeline ==");

    let bucket_of = |m: &ItchMessage| (m.timestamp_nanos().saturating_sub(first_ts)) / BUCKET_NANOS;
    let n_buckets = (bucket_of(&msgs[msgs.len() - 1]) + 1) as usize;

    let mut msg_counts = vec![0u64; n_buckets];
    let mut exec_counts = vec![0u64; n_buckets];
    let mut exec_volume = vec![0u64; n_buckets];
    let mut per_symbol: Vec<BTreeMap<u16, u64>> = vec![BTreeMap::new(); n_buckets];

    for m in msgs {
        let b = bucket_of(m) as usize;
        msg_counts[b] += 1;
        if let Some(n) = executed_shares(m) {
            exec_counts[b] += 1;
            exec_volume[b] += n as u64;
        }
        *per_symbol[b].entry(m.stock_locate()).or_default() += 1;
    }

    let focus_locate = choose_focus(msgs, symbols, focus);
    let n_sub = n_buckets * (BUCKET_NANOS / SUB_BUCKET_NANOS) as usize;
    let per_bucket_subs = (BUCKET_NANOS / SUB_BUCKET_NANOS) as usize;
    let vols: Vec<f64> = match focus_locate {
        None => vec![0.0; n_buckets],
        Some(l) => {
            let mids = focus_mids(msgs, l, first_ts, n_sub);
            (0..n_buckets)
                .map(|b| realized_vol(&mids, b * per_bucket_subs, per_bucket_subs))
                .collect()
        }
    };
    let peak_vol = vols.iter().cloned().fold(0.0f64, f64::max).max(f64::MIN_POSITIVE);
    let focus_name = focus_locate.map(|l| symbols.name(l)).unwrap_or_else(|| "—".into());

    println!(
        "  {:>4} {:>8} {:>7} {:>10} {:>10}  {:<24}",
        "t+s",
        "msgs",
        "execs",
        "exec vol",
        "busiest",
        format!("{focus_name} volatility")
    );
    for b in 0..n_buckets {
        let busiest = per_symbol[b]
            .iter()
            .max_by_key(|&(_, n)| *n)
            .map(|(l, _)| symbols.name(*l))
            .unwrap_or_else(|| "-".into());
        let bar = "#".repeat(((vols[b] / peak_vol) * 24.0).round() as usize);
        println!(
            "  {:>4} {:>8} {:>7} {:>10} {:>10}  {bar}",
            b, msg_counts[b], exec_counts[b], exec_volume[b], busiest
        );
    }
    println!();
    println!("  Volatility is the standard deviation of successive 100 ms changes in the");
    println!("  {focus_name} touch midpoint, scaled to the busiest second. Compare the shape against");
    println!("  the transmitter's own table: an opening burst, a calm middle, a shock just past");
    println!("  halfway, a ramp into the close. A different shape means messages went missing.");
}

fn executed_shares(m: &ItchMessage) -> Option<u32> {
    match m {
        ItchMessage::OrderExecuted(e) => Some(e.shares),
        ItchMessage::OrderExecutedWithPrice(e) => Some(e.shares),
        _ => None,
    }
}

/// Standard deviation of successive differences between sub-bucket mid prices —
/// a realized-volatility estimate that is insensitive to the level of the price
/// and to how many orders happened to print in a given window.
fn realized_vol(mids: &[Option<f64>], start: usize, len: usize) -> f64 {
    let means: Vec<f64> = mids[start.min(mids.len())..(start + len).min(mids.len())]
        .iter()
        .flatten()
        .copied()
        .collect();
    if means.len() < 3 {
        return 0.0;
    }
    let diffs: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    (diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / diffs.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fixtures::synthetic;

    #[test]
    fn clock_renders_the_opening_bell() {
        assert_eq!(clock(34_200_000_000_000), "09:30:00.000000000");
        assert_eq!(clock(34_200_000_100_000), "09:30:00.000100000");
        assert_eq!(clock(34_260_000_000_000), "09:31:00.000000000");
    }

    #[test]
    fn realized_vol_is_zero_for_a_flat_price_and_positive_for_a_moving_one() {
        let flat: Vec<Option<f64>> = vec![Some(100.0); 10];
        assert_eq!(realized_vol(&flat, 0, 10), 0.0);

        let moving: Vec<Option<f64>> = (0..10).map(|i| Some(100.0 + (i % 3) as f64 * 7.0)).collect();
        assert!(realized_vol(&moving, 0, 10) > 0.0);

        // Windows with no two-sided quote are skipped, not counted as zero.
        let gappy: Vec<Option<f64>> =
            moving.iter().enumerate().map(|(i, m)| if i % 2 == 0 { *m } else { None }).collect();
        assert!(realized_vol(&gappy, 0, 10) > 0.0);

        assert_eq!(realized_vol(&flat, 0, 2), 0.0);
        assert_eq!(realized_vol(&[], 0, 10), 0.0);
        assert_eq!(realized_vol(&flat, 50, 10), 0.0, "a start past the end must not panic");
    }

    #[test]
    fn the_focus_symbol_comes_from_the_data() {
        let msgs = synthetic(5_000);
        let symbols = SymbolMap::learn(&msgs);

        // A named symbol that exists is honoured.
        let named = choose_focus(&msgs, &symbols, Some("CCC"));
        assert_eq!(named, symbols.locate_of("CCC"));

        // One that does not falls back rather than failing.
        let fallback = choose_focus(&msgs, &symbols, Some("NOSUCH"));
        assert!(fallback.is_some());
        assert_eq!(fallback, choose_focus(&msgs, &symbols, None));

        assert_eq!(choose_focus(&[], &symbols, None), None);
    }

    #[test]
    fn focus_mids_uses_the_touch_and_skips_one_sided_windows() {
        let msgs = synthetic(20_000);
        let symbols = SymbolMap::learn(&msgs);
        let locate = symbols.locate_of("AAA").unwrap();
        let mids = focus_mids(&msgs, locate, msgs[0].timestamp_nanos(), 20);
        assert_eq!(mids.len(), 20);
        assert!(mids.iter().filter(|m| m.is_some()).count() > 15);
        // A locate that never traded has no mid anywhere.
        let empty = focus_mids(&msgs, 999, msgs[0].timestamp_nanos(), 20);
        assert!(empty.iter().all(|m| m.is_none()));
    }

    /// Summarising must not panic on the degenerate inputs — an empty capture,
    /// one message, or a capture with no symbol map at all.
    #[test]
    fn survives_tiny_and_anonymous_captures() {
        let symbols = SymbolMap::default();
        summarise(&[], &symbols, None);
        summarise(&synthetic(1), &symbols, None);
        summarise(&synthetic(50), &symbols, Some("AAA"));

        // No adds at all: every symbol is anonymous and priceless.
        let deletes: Vec<ItchMessage> = synthetic(2_000)
            .into_iter()
            .filter(|m| m.message_type() == b'D')
            .collect();
        assert!(!deletes.is_empty());
        summarise(&deletes, &SymbolMap::default(), None);
    }
}
