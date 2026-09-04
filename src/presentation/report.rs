//! The two loss reports, rendered.
//!
//! Both used to sit at the bottom of the modules that compute them, which put
//! `println!` inside what is now [`crate::domain::loss`]. The computation did
//! not change; only its audience moved out here.
//!
//! [`blind_spot_note`] came out with them, and it is the more interesting of
//! the two moves. It was a method on `Detection` returning a formatted English
//! sentence — a rendering concern wearing a domain type's clothes. The domain
//! now supplies `unverifiable` and `total`; the sentence is written here.

use crate::domain::loss::{Comparison, Detection, SCAN_WINDOW, SequenceGaps};

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
    println!("  BLIND SPOT      {}", blind_spot_note(d));
    println!("                  Gaps are only visible between two values that arrived, so loss");
    println!("                  before a sequence's first survivor or after its last is not");
    println!("                  counted either. In particular, if the last N messages never");
    println!("                  arrive, nothing left in the stream says so.");
    println!("                  Run with --csv to compare against the transmitter's ground truth,");
    println!("                  which has neither limitation.");
}

/// The share of the received feed for which loss would leave no trace, in
/// words.
///
/// 'D' and 'X' allocate no order reference and no match number, so nothing in
/// the stream ever contradicts their absence. Saying so is the whole reason the
/// detection report is safe to read.
pub fn blind_spot_note(d: &Detection) -> String {
    format!(
        "{} of {} received messages ({:.1}%) were 'D' or 'X', which carry neither an \
         order reference of their own nor a match number. Loss among those is invisible \
         here — a clean report is not proof of a clean stream.",
        d.unverifiable,
        d.total,
        d.blind_fraction() * 100.0,
    )
}

/// Prints a ground-truth comparison. This is the report that can actually say
/// "zero loss" and mean it.
pub fn print_comparison(c: &Comparison) {
    println!();
    println!("== verification against ground truth ==");
    println!("  expected        {} messages", c.expected);
    println!("  received        {} messages", c.received);
    println!("  matched         {} in order", c.matched);

    if c.is_perfect() {
        println!();
        println!("  RESULT          0% LOSS — every message sent arrived, in order, byte for byte.");
        return;
    }

    println!(
        "  missing         {} ({:.4}% of the feed)",
        c.missing.len(),
        c.loss_fraction() * 100.0
    );
    if !c.unexpected.is_empty() {
        println!(
            "  unexpected      {} datagrams matched nothing — corruption, reordering beyond \
             {} messages, or another sender on this port",
            c.unexpected.len(),
            SCAN_WINDOW
        );
    }

    let runs = c.gap_runs();
    if !runs.is_empty() {
        println!(
            "  gaps            {} runs, largest {} consecutive",
            runs.len(),
            c.largest_gap()
        );
        print!("  first gaps      ");
        for (i, (start, len)) in runs.iter().take(8).enumerate() {
            if i > 0 {
                print!(", ");
            }
            print!("{start}..{}", start + len);
        }
        if runs.len() > 8 {
            print!(", … {} more", runs.len() - 8);
        }
        println!();
    }
    if c.is_tail_truncation() {
        println!();
        println!("  All of it is at the end — the stream stopped early rather than dropping");
        println!("  messages throughout. A sender that died, or tail loss. This is exactly the");
        println!("  case no content-based detector can see, which is what heartbeats are for.");
    }
    println!();
    println!("  RESULT          FAILED — {} messages did not arrive.", c.missing.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fixtures::{drop_indices, synthetic};

    /// The note has to name the counts it is caveating, or it is decoration.
    #[test]
    fn the_blind_spot_note_quotes_the_numbers_it_is_about() {
        let msgs = synthetic(5_000);
        let d = Detection::run(&msgs);
        let note = blind_spot_note(&d);
        assert!(note.contains(&d.unverifiable.to_string()), "{note}");
        assert!(note.contains(&d.total.to_string()), "{note}");
        assert!(note.contains("'D' or 'X'"), "{note}");
        assert!(d.blind_fraction() > 0.2, "D and X are a large share of the tape");
    }

    /// An empty capture must render rather than divide by zero.
    #[test]
    fn an_empty_detection_still_renders() {
        let d = Detection::run(&[]);
        assert!(blind_spot_note(&d).contains("0 of 0"));
        print_detection(&d);
    }

    /// Both reports are `println!`-only, so the test that matters is that they
    /// survive every shape of input the pipeline can hand them.
    #[test]
    fn the_reports_survive_perfect_lossy_and_empty_captures() {
        let sent = synthetic(2_000);
        print_detection(&Detection::run(&sent));
        print_comparison(&crate::domain::loss::compare(&sent, &sent));

        let kept = drop_indices(&sent, &(500..900).collect::<Vec<_>>());
        print_detection(&Detection::run(&kept));
        print_comparison(&crate::domain::loss::compare(&sent, &kept));

        print_comparison(&crate::domain::loss::compare(&sent, &sent[..1_000]));
        print_comparison(&crate::domain::loss::compare(&[], &[]));
    }
}
