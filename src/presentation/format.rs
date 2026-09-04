//! Pure rendering helpers: bytes and prices into strings.
//!
//! Nothing here does I/O, and nothing here may be used by [`crate::domain`] —
//! the codec's tests used to reach up to the crate root for `hex` and
//! `format_price` to assert that a price *renders* as "150.2500", which put a
//! display concern inside an enterprise rule. Those assertions live below now,
//! where they belong.

/// Space-separated hex dump, for eyeballing a payload against `tcpdump -X`.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(if i % 16 == 0 { '\n' } else { ' ' });
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Renders an ITCH price (scaled by 10,000) as a decimal string.
///
/// Integer math only — there are no floats in ITCH and there should be none in
/// the codec. Formatting belongs at the display layer, which is here.
pub fn format_price(scaled: u32) -> String {
    format!("{}.{:04}", scaled / 10_000, scaled % 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The assertion the codec's golden-vector test used to make. The wire
    /// format owns the scaled integer 1_502_500; how it reads as $150.25 is
    /// this layer's business.
    #[test]
    fn prices_render_with_four_decimal_places() {
        assert_eq!(format_price(1_502_500), "150.2500");
        assert_eq!(format_price(0), "0.0000");
        assert_eq!(format_price(1), "0.0001");
    }

    #[test]
    fn hex_wraps_every_sixteen_bytes() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x41, 0x00, 0xFF]), "41 00 FF");
        let dump = hex(&[0u8; 17]);
        assert_eq!(dump.lines().count(), 2);
        assert_eq!(dump.lines().next().unwrap().split(' ').count(), 16);
    }
}
