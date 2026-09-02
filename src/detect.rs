//! Loss detection from the message content alone.
//!
//! Slice 2 puts nothing on the wire to detect loss with: no packet sequence, no
//! message sequence, no heartbeat, no session id. That work belongs to a session
//! layer the transmitter has not built yet (`docs/session-layer.md` in the
//! transmitter repo). So everything here is inference from the ITCH payload, and
//! it is important to be honest about exactly how far that gets:
//!
//! | evidence | catches | share of a typical feed |
//! |---|---|---|
//! | order-reference gaps | lost `A`, `F`, `U` | ~49% |
//! | match-number gaps    | lost `E`, `C`      | ~15% |
//! | dangling references  | corroborates the above | — |
//! | *nothing*            | lost `D`, `X`      | **~36%** |
//!
//! `D` and `X` allocate no reference and no match number. A lost delete leaves
//! a phantom order resting in this receiver's book forever; a lost cancel leaves
//! a share count silently high. Nothing in the stream ever contradicts either.
//! [`Detection::blind_spot_note`] says so in the report rather than letting a
//! clean-looking summary imply a clean stream.
//!
//! Nothing here hardcodes the transmitter's constants. The transmitter happens
//! to stride order references by 8 (it runs 8 symbols) and match numbers by 1,
//! but a receiver that assumes those numbers breaks the day the transmitter
//! lists a ninth symbol — silently, reporting phantom loss. So the stride is
//! *inferred* from the data, and a stream that does not fit the inferred stride
//! is reported as irregular rather than quietly rescaled.

use std::collections::{BTreeMap, HashMap};

use crate::model::ItchMessage;

/// What a monotonic counter in the stream says about what is missing from it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SequenceGaps {
    /// The step between consecutive values, inferred as the smallest positive
    /// delta observed. `None` when there were fewer than two values.
    pub stride: Option<u64>,
    /// Values that took part in the estimate.
    pub observed: u64,
    /// Values the gaps imply were lost.
    ///
    /// A gap is only visible *between* two values that arrived, so this
    /// undercounts by however many were lost before the first surviving value
    /// or after the last. Per symbol that is a handful of messages; across the
    /// whole feed it is why the tail of a stream is undetectable.
    pub missing: u64,
    /// Values that went backwards — reordering, a restart, or foreign traffic.
    /// With no session id on the wire the three are indistinguishable.
    pub backwards: u64,
    /// Deltas that were not a whole multiple of the stride. Any of these means
    /// the stride assumption is wrong and `missing` is not trustworthy.
    pub irregular: u64,
}

impl SequenceGaps {
    /// True when the sequence behaved exactly as a uniformly strided counter.
    pub fn is_clean(&self) -> bool {
        self.missing == 0 && self.backwards == 0 && self.irregular == 0
    }

    /// Infers the stride and counts the gaps in an already-ordered sequence.
    pub fn analyze(values: &[u64]) -> SequenceGaps {
        let mut gaps = SequenceGaps { observed: values.len() as u64, ..Default::default() };
        if values.len() < 2 {
            return gaps;
        }

        // The stride is the smallest positive step. Deliberately not the GCD:
        // the GCD divides every delta by construction, so it can never report
        // an irregular one, which makes it self-confirming. The minimum can be
        // wrong — and when it is, `irregular` says so.
        let mut stride = u64::MAX;
        for w in values.windows(2) {
            if w[1] > w[0] {
                stride = stride.min(w[1] - w[0]);
            }
        }
        if stride == u64::MAX {
            gaps.backwards = (values.len() - 1) as u64;
            return gaps;
        }
        gaps.stride = Some(stride);

        for w in values.windows(2) {
            if w[1] <= w[0] {
                gaps.backwards += 1;
                continue;
            }
            let delta = w[1] - w[0];
            if delta % stride != 0 {
                gaps.irregular += 1;
                continue;
            }
            gaps.missing += delta / stride - 1;
        }
        gaps
    }
}

/// What replaying the stream into an order book found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BookReport {
    /// Orders introduced by `A`, `F`, or the new half of a `U`.
    pub adds: u64,
    /// Messages naming an order this receiver never saw added — the direct
    /// symptom of a lost add.
    pub dangling: u64,
    /// An add whose reference was already live. References are never reused, so
    /// this means duplication or reordering.
    pub duplicate_refs: u64,
    /// An execution or cancel for more shares than the order was believed to
    /// hold — the symptom of a *lost* earlier add being replaced by a stale one,
    /// or of a corrupt share count.
    pub over_execution: u64,
    /// Orders still resting when the stream ended. Some are genuine; each lost
    /// `D` adds one phantom, and the two cannot be told apart.
    pub still_live: usize,
}

/// Per-symbol activity, keyed by ITCH stock locate.
#[derive(Debug, Clone, Default)]
pub struct LocateReport {
    pub ticker: Option<String>,
    pub messages: u64,
    pub adds: SequenceGaps,
}

#[derive(Debug, Clone, Default)]
pub struct Detection {
    /// Order-reference gaps, summed over every symbol.
    pub adds: SequenceGaps,
    /// Match-number gaps, over the whole feed.
    pub executions: SequenceGaps,
    /// Timestamp-grid gaps. See [`Detection::timestamps`] — a scaffold, not a
    /// loss detector.
    pub timestamps: SequenceGaps,
    pub book: BookReport,
    pub per_locate: BTreeMap<u16, LocateReport>,
    /// Messages that carry no sequence evidence of any kind: `D` and `X`.
    pub unverifiable: u64,
    pub total: u64,
}

impl Detection {
    /// Messages this can prove were lost, from sequence evidence alone.
    pub fn provable_loss(&self) -> u64 {
        self.adds.missing + self.executions.missing
    }

    /// The share of the received feed for which loss would leave no trace.
    pub fn blind_fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.unverifiable as f64 / self.total as f64
        }
    }

    pub fn blind_spot_note(&self) -> String {
        format!(
            "{} of {} received messages ({:.1}%) were 'D' or 'X', which carry neither an \
             order reference of their own nor a match number. Loss among those is invisible \
             here — a clean report is not proof of a clean stream.",
            self.unverifiable,
            self.total,
            self.blind_fraction() * 100.0,
        )
    }

    pub fn run(msgs: &[ItchMessage]) -> Detection {
        let mut d = Detection { total: msgs.len() as u64, ..Default::default() };

        // Sequences, gathered in arrival order.
        let mut refs_by_locate: BTreeMap<u16, Vec<u64>> = BTreeMap::new();
        let mut match_numbers: Vec<u64> = Vec::new();
        let mut timestamps: Vec<u64> = Vec::with_capacity(msgs.len());

        // Book replay state: reference -> (locate, shares believed resting).
        let mut book: HashMap<u64, (u16, u32)> = HashMap::new();

        for msg in msgs {
            let locate = msg.stock_locate();
            let entry = d.per_locate.entry(locate).or_default();
            entry.messages += 1;
            timestamps.push(msg.timestamp_nanos());

            match msg {
                ItchMessage::AddOrder(m) => {
                    entry.ticker.get_or_insert_with(|| ticker_of(&{ m.stock }));
                    refs_by_locate.entry(locate).or_default().push(m.order_reference);
                    add_to_book(&mut book, &mut d.book, m.order_reference, locate, m.shares);
                }
                ItchMessage::AddOrderAttributed(m) => {
                    entry.ticker.get_or_insert_with(|| ticker_of(&{ m.stock }));
                    refs_by_locate.entry(locate).or_default().push(m.order_reference);
                    add_to_book(&mut book, &mut d.book, m.order_reference, locate, m.shares);
                }
                ItchMessage::OrderReplace(m) => {
                    // Replace is a delete and an add fused into one message.
                    // Treating it as an in-place edit leaks the old reference.
                    if book.remove(&{ m.original_order_reference }).is_none() {
                        d.book.dangling += 1;
                    }
                    refs_by_locate.entry(locate).or_default().push(m.new_order_reference);
                    add_to_book(&mut book, &mut d.book, m.new_order_reference, locate, m.shares);
                }
                ItchMessage::OrderExecuted(m) => {
                    match_numbers.push(m.match_number);
                    reduce(&mut book, &mut d.book, m.order_reference, m.shares, true);
                }
                ItchMessage::OrderExecutedWithPrice(m) => {
                    match_numbers.push(m.match_number);
                    reduce(&mut book, &mut d.book, m.order_reference, m.shares, true);
                }
                ItchMessage::OrderCancel(m) => {
                    d.unverifiable += 1;
                    reduce(&mut book, &mut d.book, m.order_reference, m.canceled_shares, false);
                }
                ItchMessage::OrderDelete(m) => {
                    d.unverifiable += 1;
                    if book.remove(&{ m.order_reference }).is_none() {
                        d.book.dangling += 1;
                    }
                }
            }
        }

        for (locate, refs) in &refs_by_locate {
            let gaps = SequenceGaps::analyze(refs);
            d.adds.observed += gaps.observed;
            d.adds.missing += gaps.missing;
            d.adds.backwards += gaps.backwards;
            d.adds.irregular += gaps.irregular;
            // Every symbol allocates from the same stride; disagreement means
            // the assumption is wrong, so keep the largest as the reported one
            // and let `irregular` carry the doubt.
            d.adds.stride = match (d.adds.stride, gaps.stride) {
                (None, s) => s,
                (Some(a), Some(b)) => Some(a.max(b)),
                (s, None) => s,
            };
            d.per_locate.entry(*locate).or_default().adds = gaps;
        }

        d.executions = SequenceGaps::analyze(&match_numbers);
        d.timestamps = SequenceGaps::analyze(&timestamps);
        d.book.still_live = book.len();
        d
    }
}

fn ticker_of(field: &[u8; 8]) -> String {
    crate::model::unpack_stock_symbol(field).to_string()
}

fn add_to_book(
    book: &mut HashMap<u64, (u16, u32)>,
    report: &mut BookReport,
    reference: u64,
    locate: u16,
    shares: u32,
) {
    report.adds += 1;
    if book.insert(reference, (locate, shares)).is_some() {
        report.duplicate_refs += 1;
    }
}

/// Applies an execution or a cancel to the book.
fn reduce(
    book: &mut HashMap<u64, (u16, u32)>,
    report: &mut BookReport,
    reference: u64,
    shares: u32,
    remove_when_empty: bool,
) {
    match book.get_mut(&reference) {
        None => report.dangling += 1,
        Some((_, held)) => {
            if shares > *held {
                report.over_execution += 1;
                *held = 0;
            } else {
                *held -= shares;
            }
            if *held == 0 && remove_when_empty {
                book.remove(&reference);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{drop_every, drop_indices, synthetic, INTERVAL_NANOS, LOCATES};
    use std::collections::BTreeMap;

    #[test]
    fn a_complete_stream_reports_no_loss_anywhere() {
        let d = Detection::run(&synthetic(20_000));
        assert!(d.adds.is_clean(), "{:?}", d.adds);
        assert!(d.executions.is_clean(), "{:?}", d.executions);
        assert!(d.timestamps.is_clean(), "{:?}", d.timestamps);
        assert_eq!(d.book.dangling, 0);
        assert_eq!(d.book.duplicate_refs, 0);
        assert_eq!(d.book.over_execution, 0);
        assert_eq!(d.provable_loss(), 0);
    }

    /// The stride must come from the data. The fixture uses 4 symbols where the
    /// transmitter uses 8; a receiver that hardcoded 8 would report phantom loss
    /// on every single add.
    #[test]
    fn the_stride_is_inferred_not_assumed() {
        let d = Detection::run(&synthetic(10_000));
        assert_eq!(d.adds.stride, Some(LOCATES), "add stride should be inferred as 4");
        assert_eq!(d.executions.stride, Some(1), "match numbers stride by 1");
        assert_eq!(d.timestamps.stride, Some(INTERVAL_NANOS));
    }

    /// Exact counts, not estimates — within the one limit the technique has.
    ///
    /// A gap needs a value on both sides of it. An add lost before a symbol's
    /// first surviving add, or after its last, leaves no gap to see. So the
    /// assertion is not "missing == dropped" but "missing == exactly the
    /// dropped adds that fall inside an observable range", which is the strongest
    /// true statement available.
    #[test]
    fn add_and_execution_gaps_count_the_true_loss_exactly() {
        use crate::model::ItchMessage as M;
        let full = synthetic(20_000);
        let (kept, dropped) = drop_every(&full, 10);

        let add_ref = |m: &M| -> Option<(u16, u64)> {
            match m {
                M::AddOrder(a) => Some((a.stock_locate, a.order_reference)),
                M::AddOrderAttributed(a) => Some((a.stock_locate, a.order_reference)),
                M::OrderReplace(u) => Some((u.stock_locate, u.new_order_reference)),
                _ => None,
            }
        };
        // The reference range each symbol's surviving adds actually span.
        let mut lo: BTreeMap<u16, u64> = BTreeMap::new();
        let mut hi: BTreeMap<u16, u64> = BTreeMap::new();
        for m in &kept {
            if let Some((l, r)) = add_ref(m) {
                lo.entry(l).or_insert(r);
                hi.insert(l, r);
            }
        }

        let mut detectable_adds = 0u64;
        let mut boundary_adds = 0u64;
        for &i in &dropped {
            if let Some((l, r)) = add_ref(&full[i]) {
                if r > lo[&l] && r < hi[&l] {
                    detectable_adds += 1;
                } else {
                    boundary_adds += 1;
                }
            }
        }
        let truth_execs = dropped
            .iter()
            .filter(|&&i| matches!(full[i].message_type(), b'E' | b'C'))
            .count() as u64;

        let d = Detection::run(&kept);
        assert_eq!(d.adds.missing, detectable_adds, "add loss must be exact inside the range");
        assert_eq!(d.executions.missing, truth_execs, "execution loss must be exact");
        assert_eq!(d.adds.irregular, 0, "a real gap is still a multiple of the stride");
        // The boundary cost is small but real, and must not be pretended away.
        assert!(boundary_adds > 0, "this fixture should exercise the boundary case");
        assert!(
            boundary_adds < detectable_adds / 100,
            "boundary losses should be a rounding error, got {boundary_adds} of {detectable_adds}"
        );
    }

    /// The boundary limit on its own, stated as a property rather than left as
    /// an off-by-one someone rediscovers later.
    #[test]
    fn loss_at_the_edge_of_a_sequence_leaves_no_gap_to_find() {
        // Only the last value of the sequence is gone: nothing follows it, so
        // nothing is missing as far as the arithmetic can tell.
        let gaps = SequenceGaps::analyze(&[10, 20, 30]);
        assert_eq!(gaps.missing, 0);
        // The same three values with the middle one gone is plainly detectable.
        let gaps = SequenceGaps::analyze(&[10, 30]);
        assert_eq!(gaps.stride, Some(20), "with one sample of the delta, 20 looks like the stride");

        // At feed scale: dropping a symbol's very first add hides it entirely.
        let full = synthetic(5_000);
        let first_add = full.iter().position(|m| m.message_type() == b'A').unwrap();
        let locate = full[first_add].stock_locate();
        let is_first = !full[..first_add].iter().any(|m| m.stock_locate() == locate);
        assert!(is_first);
        let d = Detection::run(&drop_indices(&full, &[first_add]));
        assert_eq!(d.adds.missing, 0, "nothing precedes the first add, so no gap opens");
        // The book replay still notices, which is why both run.
        assert!(d.book.dangling > 0, "the order's later messages have nothing to attach to");
    }

    /// The honest half: deletes and cancels leave no trace at all.
    #[test]
    fn deletes_and_cancels_are_lost_silently() {
        let full = synthetic(20_000);
        let victims: Vec<usize> = (0..full.len())
            .filter(|&i| matches!(full[i].message_type(), b'D' | b'X'))
            .take(200)
            .collect();
        assert_eq!(victims.len(), 200, "the fixture should contain plenty of D/X");

        let kept = drop_indices(&full, &victims);
        let d = Detection::run(&kept);
        assert_eq!(d.provable_loss(), 0, "200 messages vanished and nothing noticed");
        assert_eq!(d.book.dangling, 0, "a lost delete leaves no dangling reference either");
        // The only trace is indirect: the book ends up holding phantoms.
        let clean = Detection::run(&full);
        assert!(
            d.book.still_live > clean.book.still_live,
            "lost deletes should leave phantom orders resting: {} vs {}",
            d.book.still_live,
            clean.book.still_live
        );
        assert!(d.blind_fraction() > 0.2, "D and X are a large share of the tape");
    }

    /// A lost add turns every later message about that order into a dangling
    /// reference — the corroborating symptom, and the one that works even when
    /// the sequence arithmetic is at a boundary.
    #[test]
    fn a_lost_add_shows_up_as_both_a_gap_and_dangling_references() {
        let full = synthetic(5_000);
        // An add from the middle of the stream, so it has neighbours on both
        // sides of it in its symbol's reference sequence.
        let victim = full
            .iter()
            .enumerate()
            .filter(|(i, m)| *i > 1_000 && *i < 4_000 && m.message_type() == b'A')
            .map(|(i, _)| i)
            .next()
            .unwrap();
        let d = Detection::run(&drop_indices(&full, &[victim]));
        assert_eq!(d.adds.missing, 1, "a gap opens in that symbol's reference sequence");
        assert!(d.book.dangling > 0, "and its later messages have nothing to attach to");
    }

    #[test]
    fn tail_loss_is_invisible_to_every_detector() {
        let full = synthetic(20_000);
        let d = Detection::run(&full[..full.len() - 500]);
        assert_eq!(d.provable_loss(), 0);
        assert_eq!(d.book.dangling, 0);
        assert!(d.timestamps.is_clean());
        // Which is precisely the argument for heartbeats and an end-of-session
        // flag: 500 messages disappeared and the stream looks perfect.
    }

    #[test]
    fn reordering_is_reported_as_backwards_not_as_loss() {
        let mut msgs = synthetic(2_000);
        msgs.swap(1_500, 1_800);
        let d = Detection::run(&msgs);
        assert!(
            d.adds.backwards > 0 || d.executions.backwards > 0 || d.timestamps.backwards > 0,
            "a swap must show up as a backwards step"
        );
    }

    #[test]
    fn analyze_handles_the_degenerate_inputs() {
        assert_eq!(SequenceGaps::analyze(&[]), SequenceGaps::default());
        assert_eq!(SequenceGaps::analyze(&[7]).observed, 1);
        assert_eq!(SequenceGaps::analyze(&[7]).stride, None);

        let clean = SequenceGaps::analyze(&[10, 20, 30, 40]);
        assert_eq!(clean.stride, Some(10));
        assert_eq!(clean.missing, 0);

        let gappy = SequenceGaps::analyze(&[10, 20, 50, 60]);
        assert_eq!(gappy.stride, Some(10));
        assert_eq!(gappy.missing, 2);

        // A delta that is not a multiple of the stride means the stride guess is
        // wrong; say so rather than inventing a fractional loss count.
        let odd = SequenceGaps::analyze(&[10, 20, 35, 45]);
        assert_eq!(odd.stride, Some(10));
        assert_eq!(odd.irregular, 1);

        let back = SequenceGaps::analyze(&[10, 20, 15, 25]);
        assert_eq!(back.backwards, 1);

        // Strictly decreasing: no positive delta exists to infer a stride from.
        let down = SequenceGaps::analyze(&[30, 20, 10]);
        assert_eq!(down.stride, None);
        assert_eq!(down.backwards, 2);
    }

    /// Uniform loss can defeat stride inference, and that is worth knowing
    /// rather than pretending otherwise: if *every* gap is the same size, the
    /// smallest delta is the gap.
    #[test]
    fn perfectly_uniform_loss_defeats_the_inference() {
        let gaps = SequenceGaps::analyze(&[10, 30, 50, 70]);
        assert_eq!(gaps.stride, Some(20), "with every other value gone, 20 looks like the stride");
        assert_eq!(gaps.missing, 0, "and so nothing looks missing — a real limit of inference");
    }

}

/// Prints a detection report.
///
/// Ordered so the caveat cannot be missed: what was proven, then what could not
/// be. A report that led with "no loss detected" would be actively misleading.
pub fn print_detection(d: &Detection) {
    println!();
    println!("== loss detection (from message content alone) ==");

    let line = |name: &str, g: &SequenceGaps| {
        let stride = g.stride.map(|s| s.to_string()).unwrap_or_else(|| "?".into());
        println!(
            "  {name:<22} {:>8} seen, stride {stride:>7}, {:>7} missing, {} backwards, {} irregular",
            g.observed, g.missing, g.backwards, g.irregular
        );
    };
    line("order references", &d.adds);
    line("match numbers", &d.executions);
    line("timestamp grid", &d.timestamps);

    if d.timestamps.irregular > 0 {
        println!("    timestamps are not on a uniform grid — that estimator does not apply here.");
        println!("    (It only works because this generator emits on a fixed 100 µs schedule;");
        println!("     real ITCH timestamps are event times and this column would be noise.)");
    }

    println!();
    println!("  book replay          {} adds, {} dangling refs, {} duplicate refs, {} over-executions",
        d.book.adds, d.book.dangling, d.book.duplicate_refs, d.book.over_execution);
    println!("  orders left resting  {}", d.book.still_live);

    println!();
    if d.provable_loss() == 0 {
        println!("  PROVABLE LOSS   none — every sequence is intact.");
    } else {
        println!(
            "  PROVABLE LOSS   {} messages ({} adds, {} executions)",
            d.provable_loss(),
            d.adds.missing,
            d.executions.missing
        );
    }
    println!("  BLIND SPOT      {}", d.blind_spot_note());
    println!("                  Gaps are only visible between two values that arrived, so loss");
    println!("                  before a sequence's first survivor or after its last is not");
    println!("                  counted either. In particular, if the last N messages never");
    println!("                  arrive, nothing left in the stream says so.");
    println!("                  Run with --csv to compare against the transmitter's ground truth,");
    println!("                  which has neither limitation.");
}
