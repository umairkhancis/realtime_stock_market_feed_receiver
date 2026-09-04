//! Deterministic synthetic streams, for tests only.
//!
//! The receiver has no market generator and should not grow one — that is the
//! transmitter's job, and a receiver that can only be tested against its own
//! idea of a feed is testing nothing. But the detectors in
//! [`crate::domain::loss`] need a stream with known properties *and* a known
//! amount of loss injected into it, so this builds one.
//!
//! It sits in `domain` because it builds nothing but domain values, and it is
//! `#[cfg(test)]` because a receiver that ships a feed generator is a receiver
//! that will eventually be tested against itself. That is also why the two
//! cross-layer properties this crate cares about stay unit tests rather than
//! moving to `tests/`: promoting them would mean publishing this module.
//!
//! Deliberately unlike the transmitter in one way: four symbols, so order
//! references stride by 4 rather than 8. Nothing in the receiver may hardcode
//! the transmitter's stride — it has to infer it. These fixtures are what
//! proves it does.

use crate::domain::message::{
    ItchAddOrder, ItchAddOrderAttributed, ItchMessage, ItchOrderCancel, ItchOrderDelete,
    ItchOrderExecuted, ItchOrderExecutedWithPrice, ItchOrderReplace, pack_itch_timestamp,
    pack_stock_symbol,
};

pub const SESSION_OPEN_NANOS: u64 = 34_200_000_000_000;
pub const INTERVAL_NANOS: u64 = 100_000;
/// Symbols in the fixture universe, and therefore the order-reference stride.
pub const LOCATES: u64 = 4;
pub const TICKERS: [&str; 4] = ["AAA", "BBB", "CCC", "DDD"];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*, enough for a fixture.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Copy)]
struct Live {
    reference: u64,
    side: u8,
    shares: u32,
    price: u32,
}

/// A stream of `count` messages that satisfies every invariant the detectors
/// look for: per-locate references striding by [`LOCATES`], match numbers
/// striding by 1, timestamps on a uniform grid, and a book in which every
/// reference is live when it is used.
pub fn synthetic(count: usize) -> Vec<ItchMessage> {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut next_seq = [0u64; LOCATES as usize];
    let mut live: Vec<Vec<Live>> = vec![Vec::new(); LOCATES as usize];
    let mut next_match = 1u64;
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let s = rng.below(LOCATES) as usize;
        let locate = (s + 1) as u16;
        let ts = pack_itch_timestamp(SESSION_OPEN_NANOS + i as u64 * INTERVAL_NANOS)
            .expect("fixture session fits in 48 bits");
        let stock = pack_stock_symbol(TICKERS[s]).expect("fixture tickers are valid");

        let mut new_reference = || {
            next_seq[s] += 1;
            next_seq[s] * LOCATES + locate as u64
        };

        // Keep a floor of resting orders so there is always something to hit.
        let kind = if live[s].len() < 4 { 0 } else { rng.below(7) };
        let msg = match kind {
            0 => {
                let o = Live {
                    reference: new_reference(),
                    side: if rng.below(2) == 0 { b'B' } else { b'S' },
                    shares: 100 * (1 + rng.below(9) as u32),
                    price: 1_000_000 + 100 * rng.below(500) as u32,
                };
                live[s].push(o);
                ItchMessage::AddOrder(ItchAddOrder {
                    message_type: b'A',
                    stock_locate: locate,
                    tracking_number: 0,
                    timestamp_bytes: ts,
                    order_reference: o.reference,
                    buy_sell_indicator: o.side,
                    shares: o.shares,
                    stock,
                    price: o.price,
                })
            }
            1 => {
                let o = Live {
                    reference: new_reference(),
                    side: if rng.below(2) == 0 { b'B' } else { b'S' },
                    shares: 100 * (1 + rng.below(9) as u32),
                    price: 1_000_000 + 100 * rng.below(500) as u32,
                };
                live[s].push(o);
                ItchMessage::AddOrderAttributed(ItchAddOrderAttributed {
                    message_type: b'F',
                    stock_locate: locate,
                    tracking_number: 0,
                    timestamp_bytes: ts,
                    order_reference: o.reference,
                    buy_sell_indicator: o.side,
                    shares: o.shares,
                    stock,
                    price: o.price,
                    attribution: *b"TEST",
                })
            }
            2 | 3 => {
                let j = rng.below(live[s].len() as u64) as usize;
                let o = live[s][j];
                let executed = if o.shares <= 100 || rng.below(3) == 0 {
                    live[s].swap_remove(j);
                    o.shares
                } else {
                    let n = 100 * (1 + rng.below((o.shares / 100 - 1).max(1) as u64) as u32);
                    let n = n.min(o.shares - 1);
                    live[s][j].shares -= n;
                    n
                };
                let match_number = next_match;
                next_match += 1;
                if kind == 2 {
                    ItchMessage::OrderExecuted(ItchOrderExecuted {
                        message_type: b'E',
                        stock_locate: locate,
                        tracking_number: 0,
                        timestamp_bytes: ts,
                        order_reference: o.reference,
                        shares: executed,
                        match_number,
                    })
                } else {
                    ItchMessage::OrderExecutedWithPrice(ItchOrderExecutedWithPrice {
                        message_type: b'C',
                        stock_locate: locate,
                        tracking_number: 0,
                        timestamp_bytes: ts,
                        order_reference: o.reference,
                        shares: executed,
                        match_number,
                        printable: b'Y',
                        execution_price: o.price,
                    })
                }
            }
            4 => {
                let j = rng.below(live[s].len() as u64) as usize;
                let o = live[s][j];
                if o.shares < 200 {
                    live[s].swap_remove(j);
                    ItchMessage::OrderDelete(ItchOrderDelete {
                        message_type: b'D',
                        stock_locate: locate,
                        tracking_number: 0,
                        timestamp_bytes: ts,
                        order_reference: o.reference,
                    })
                } else {
                    let canceled = 100;
                    live[s][j].shares -= canceled;
                    ItchMessage::OrderCancel(ItchOrderCancel {
                        message_type: b'X',
                        stock_locate: locate,
                        tracking_number: 0,
                        timestamp_bytes: ts,
                        order_reference: o.reference,
                        canceled_shares: canceled,
                    })
                }
            }
            5 => {
                let j = rng.below(live[s].len() as u64) as usize;
                let o = live[s].swap_remove(j);
                ItchMessage::OrderDelete(ItchOrderDelete {
                    message_type: b'D',
                    stock_locate: locate,
                    tracking_number: 0,
                    timestamp_bytes: ts,
                    order_reference: o.reference,
                })
            }
            _ => {
                let j = rng.below(live[s].len() as u64) as usize;
                let old = live[s].swap_remove(j);
                let o = Live {
                    reference: new_reference(),
                    side: old.side,
                    shares: 100 * (1 + rng.below(9) as u32),
                    price: 1_000_000 + 100 * rng.below(500) as u32,
                };
                live[s].push(o);
                ItchMessage::OrderReplace(ItchOrderReplace {
                    message_type: b'U',
                    stock_locate: locate,
                    tracking_number: 0,
                    timestamp_bytes: ts,
                    original_order_reference: old.reference,
                    new_order_reference: o.reference,
                    shares: o.shares,
                    price: o.price,
                })
            }
        };
        out.push(msg);
    }
    out
}

/// Drops every message whose index is in `drop`, simulating loss.
pub fn drop_indices(msgs: &[ItchMessage], drop: &[usize]) -> Vec<ItchMessage> {
    msgs.iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, m)| *m)
        .collect()
}

/// Drops one message in every `every`, and reports which indices went.
pub fn drop_every(msgs: &[ItchMessage], every: usize) -> (Vec<ItchMessage>, Vec<usize>) {
    let dropped: Vec<usize> = (0..msgs.len()).filter(|i| i % every == every - 1).collect();
    (drop_indices(msgs, &dropped), dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// The fixture is only useful if it is as consistent as the real feed.
    #[test]
    fn the_fixture_satisfies_the_invariants_it_is_meant_to_test() {
        let msgs = synthetic(20_000);
        assert_eq!(msgs.len(), 20_000);

        // Timestamps on a uniform grid.
        for (i, m) in msgs.iter().enumerate() {
            assert_eq!(m.timestamp_nanos(), SESSION_OPEN_NANOS + i as u64 * INTERVAL_NANOS);
        }

        // Per-locate references stride by LOCATES, monotonically.
        let mut last: HashMap<u16, u64> = HashMap::new();
        let mut seen: HashSet<u64> = HashSet::new();
        // Match numbers stride by 1 from 1.
        let mut expect_match = 1u64;
        // And the book never dangles.
        let mut book: HashMap<u64, u32> = HashMap::new();

        // A free fn rather than a closure: the 'U' arm has to touch 
        // before adding, and a closure capturing it would hold the borrow.
        fn note_add(
            seen: &mut HashSet<u64>,
            last: &mut HashMap<u16, u64>,
            book: &mut HashMap<u64, u32>,
            i: usize,
            r: u64,
            locate: u16,
            shares: u32,
        ) {
            assert!(seen.insert(r), "reference {r} reused at {i}");
            if let Some(prev) = last.insert(locate, r) {
                assert_eq!(r - prev, LOCATES, "stride broken at {i}");
            }
            book.insert(r, shares);
        }

        for (i, m) in msgs.iter().enumerate() {
            match m {
                ItchMessage::AddOrder(a) => {
                    note_add(&mut seen, &mut last, &mut book, i, a.order_reference, a.stock_locate, a.shares)
                }
                ItchMessage::AddOrderAttributed(a) => {
                    note_add(&mut seen, &mut last, &mut book, i, a.order_reference, a.stock_locate, a.shares)
                }
                ItchMessage::OrderReplace(u) => {
                    assert!(book.remove(&{ u.original_order_reference }).is_some(), "dangling at {i}");
                    note_add(&mut seen, &mut last, &mut book, i, u.new_order_reference, u.stock_locate, u.shares);
                }
                ItchMessage::OrderExecuted(e) => {
                    assert_eq!({ e.match_number }, expect_match, "match gap at {i}");
                    expect_match += 1;
                    let held = book.get_mut(&{ e.order_reference }).expect("dangling");
                    assert!(e.shares <= *held);
                    *held -= e.shares;
                    if *held == 0 {
                        book.remove(&{ e.order_reference });
                    }
                }
                ItchMessage::OrderExecutedWithPrice(e) => {
                    assert_eq!({ e.match_number }, expect_match, "match gap at {i}");
                    expect_match += 1;
                    let held = book.get_mut(&{ e.order_reference }).expect("dangling");
                    assert!(e.shares <= *held);
                    *held -= e.shares;
                    if *held == 0 {
                        book.remove(&{ e.order_reference });
                    }
                }
                ItchMessage::OrderCancel(x) => {
                    let held = book.get_mut(&{ x.order_reference }).expect("dangling");
                    assert!(x.canceled_shares < *held, "a cancel must not empty an order");
                    *held -= x.canceled_shares;
                }
                ItchMessage::OrderDelete(d) => {
                    assert!(book.remove(&{ d.order_reference }).is_some(), "dangling at {i}");
                }
            }
        }
        assert!(!book.is_empty());
    }

    #[test]
    fn the_fixture_is_deterministic() {
        assert_eq!(synthetic(2_000), synthetic(2_000));
    }

    #[test]
    fn dropping_removes_exactly_what_it_says() {
        let msgs = synthetic(1_000);
        let (kept, dropped) = drop_every(&msgs, 10);
        assert_eq!(dropped.len(), 100);
        assert_eq!(kept.len(), 900);
        for &i in &dropped {
            assert_eq!(i % 10, 9);
        }
        assert_eq!(drop_indices(&msgs, &[]).len(), 1_000);
    }
}
