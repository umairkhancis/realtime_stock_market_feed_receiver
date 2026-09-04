//! What a monotonic counter in the stream says about what is missing from it.
//!
//! Pulled out of [`super::detect`] because it is a self-contained rule with its
//! own failure modes, and because both of that module's estimators — order
//! references per symbol, match numbers across the feed — are the same
//! arithmetic over different columns. Testing it once, directly, is cheaper and
//! sharper than testing it twice through a book replay.
//!
//! Nothing here hardcodes the transmitter's constants. The transmitter happens
//! to stride order references by 8 (it runs 8 symbols) and match numbers by 1,
//! but a receiver that assumes those numbers breaks the day the transmitter
//! lists a ninth symbol — silently, reporting phantom loss. So the stride is
//! *inferred* from the data, and a stream that does not fit the inferred stride
//! is reported as irregular rather than quietly rescaled.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_handles_the_degenerate_inputs() {
        assert_eq!(SequenceGaps::analyze(&[]), SequenceGaps::default());
        assert_eq!(SequenceGaps::analyze(&[7]).observed, 1);
        assert_eq!(SequenceGaps::analyze(&[7]).stride, None);

        let clean = SequenceGaps::analyze(&[10, 20, 30, 40]);
        assert_eq!(clean.stride, Some(10));
        assert_eq!(clean.missing, 0);
        assert!(clean.is_clean());

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
