//! The locate → ticker map, and the rule for building one without help.
//!
//! Reference data, in the same sense as the transmitter's `domain/market/
//! symbols.rs` — but with one difference that is a domain rule rather than an
//! oversight. The transmitter *owns* the symbol universe: eight tickers, known
//! statically. The receiver owns nothing of the kind and must not pretend to.
//! Every ITCH message carries a `stock_locate`; only 'A' and 'F' carry the
//! ASCII ticker. So a receiver either reads the map out of band or learns it
//! from the add stream, and [`SymbolMap`] is the type that holds it either way.
//!
//! It lives in `domain` and not `infrastructure` because *which messages carry
//! a ticker* is a fact about ITCH, not about CSV. Parsing the transmitter's
//! `feed.symbols.csv` is the detail, and that half sits out in
//! [`crate::infrastructure::csv::serde::read_symbol_table`].

use std::collections::BTreeMap;

use crate::domain::message::{ItchMessage, unpack_stock_symbol};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fixtures::{TICKERS, synthetic};

    #[test]
    fn a_symbol_map_can_be_learned_from_the_adds_alone() {
        let msgs = synthetic(5_000);
        let map = SymbolMap::learn(&msgs);
        assert_eq!(map.len(), TICKERS.len());
        for (i, ticker) in TICKERS.iter().enumerate() {
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
        assert_eq!(map.len(), 0);
        assert_eq!(map.name(7), "locate:7");
    }

    #[test]
    fn an_explicit_map_names_and_reverse_names() {
        let map = SymbolMap::new(BTreeMap::from([(1, "AAPL".to_string()), (3, "NVDA".to_string())]));
        assert_eq!(map.name(3), "NVDA");
        assert_eq!(map.locate_of("NVDA"), Some(3));
        // An unknown locate is named, not guessed at and not a panic.
        assert_eq!(map.name(99), "locate:99");
        assert_eq!(map.locate_of("TSLA"), None);
    }
}
