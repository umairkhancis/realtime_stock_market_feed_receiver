
// ==========================================
// 1. ADD ORDER MESSAGES
// ==========================================

/// Message Type 'A' - Add Order (Anonymous)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchAddOrder {
    pub message_type: u8,       // 'A'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub order_reference: u64,       // Unique order ID
    pub buy_sell_indicator: u8, // 'B' = Buy, 'S' = Sell
    pub shares: u32,                // Quantity
    pub stock: [u8; 8],            // Right-padded ASCII symbol
    pub price: u32,                 // Price scaled by 10,000
}

/// Message Type 'F' - Add Order (Attributed / Shows MPID)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchAddOrderAttributed {
    pub message_type: u8,       // 'F'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub order_reference: u64,       // Unique order ID
    pub buy_sell_indicator: u8, // 'B' = Buy, 'S' = Sell
    pub shares: u32,                // Quantity
    pub stock: [u8; 8],            // Right-padded ASCII symbol
    pub price: u32,                 // Price scaled by 10,000
    pub attribution: [u8; 4],       // Market Participant Identifier (MPID)
}

// ==========================================
// 2. ORDER EXECUTION MESSAGES
// ==========================================

/// Message Type 'E' - Order Executed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchOrderExecuted {
    pub message_type: u8,       // 'E'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub order_reference: u64,       // Matches original Add Order reference
    pub shares: u32,                // Quantity executed
    pub match_number: u64,          // Unique match execution ID
}

/// Message Type 'C' - Order Executed With Price
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchOrderExecutedWithPrice {
    pub message_type: u8,       // 'C'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub order_reference: u64,       // Matches original Add Order reference
    pub shares: u32,                // Quantity executed
    pub match_number: u64,          // Unique match execution ID
    pub printable: u8,          // 'Y' = Publicly visible, 'N' = Hidden
    pub execution_price: u32,       // Non-standard execution price scaled by 10,000
}

// ==========================================
// 3. ORDER MODIFICATION MESSAGES
// ==========================================

/// Message Type 'X' - Order Cancel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchOrderCancel {
    pub message_type: u8,       // 'X'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub order_reference: u64,       // Active order ID
    pub canceled_shares: u32,       // Number of shares removed
}

/// Message Type 'D' - Order Delete
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchOrderDelete {
    pub message_type: u8,       // 'D'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub order_reference: u64,       // Active order ID to drop completely
}

/// Message Type 'U' - Order Replace
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct ItchOrderReplace {
    pub message_type: u8,       // 'U'
    pub stock_locate: u16,          // Numeric ticker identifier
    pub tracking_number: u16,       // Internal tracking
    pub timestamp_bytes: [u8; 6],   // Nanoseconds since midnight (48-bit)
    pub original_order_reference: u64, // Old order ID being modified
    pub new_order_reference: u64,   // New unique identifier for this placement
    pub shares: u32,                // Updated total quantity
    pub price: u32,                 // Updated price scaled by 10,000
}

/// Unpacks a 48-bit (6-byte) ITCH timestamp into a standard u64 register.
#[inline(always)]
pub fn unpack_itch_timestamp(bytes: &[u8; 6]) -> u64 {
    ((bytes[0] as u64) << 40) |
    ((bytes[1] as u64) << 32) |
    ((bytes[2] as u64) << 24) |
    ((bytes[3] as u64) << 16) |
    ((bytes[4] as u64) << 8)  |
    (bytes[5] as u64)
}

/// Packs nanoseconds-since-midnight into ITCH's 48-bit (6-byte) big-endian
/// timestamp. The mirror of [`unpack_itch_timestamp`].
///
/// The six bytes are the *last* six of the big-endian u64, not the first six:
///
/// ```text
/// 34_200_000_000_000u64.to_be_bytes() == [00, 00, 1F, 1A, CE, D9, F0, 00]
///                                                 ^^^^^^^^^^^^^^^^^^^^^^
/// ```
///
/// Slicing `[0..6]` compiles, runs, and yields a timestamp off by a factor of
/// 65,536 with no panic. Returns `None` for values that do not fit in 48 bits
/// (rather than silently truncating) — nanos-since-midnight tops out at
/// 86,400,000,000,000, so an overflow means an upstream bug handed us
/// nanos-since-epoch.
#[inline(always)]
pub fn pack_itch_timestamp(nanos: u64) -> Option<[u8; 6]> {
    if nanos >= 1u64 << 48 {
        return None;
    }
    let b = nanos.to_be_bytes();
    Some([b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Right-pads an ASCII symbol into ITCH's 8-byte stock field.
///
/// Padding is spaces (0x20), *not* NULs — a conforming receiver trims spaces.
/// Returns `None` if the symbol is longer than 8 bytes or is not printable ASCII.
pub fn pack_stock_symbol(symbol: &str) -> Option<[u8; 8]> {
    let bytes = symbol.as_bytes();
    if bytes.len() > 8 || !bytes.iter().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let mut out = [b' '; 8];
    out[..bytes.len()].copy_from_slice(bytes);
    Some(out)
}

/// Trims the trailing space padding off an 8-byte stock field.
pub fn unpack_stock_symbol(field: &[u8; 8]) -> &str {
    let end = field.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    std::str::from_utf8(&field[..end]).unwrap_or("<invalid utf8>")
}

// ==========================================
// 4. THE SUM TYPE
// ==========================================

/// One ITCH message of any type this transmitter speaks.
///
/// The seven structs above are a *record* of what NASDAQ specifies; this enum
/// is what the codec, the generator and the CSV layer actually pass around. It
/// exists because "an ITCH message" has to be a single type before you can
/// write `encode(msg: &ItchMessage)` or hold a heterogeneous run of them.
///
/// The largest variant is 40 bytes on the wire, so the enum lands at 48 with
/// discriminant and padding — irrelevant next to a UDP datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItchMessage {
    AddOrder(ItchAddOrder),
    AddOrderAttributed(ItchAddOrderAttributed),
    OrderExecuted(ItchOrderExecuted),
    OrderExecutedWithPrice(ItchOrderExecutedWithPrice),
    OrderCancel(ItchOrderCancel),
    OrderDelete(ItchOrderDelete),
    OrderReplace(ItchOrderReplace),
}

impl ItchMessage {
    /// The leading type byte, which is what a receiver dispatches on.
    pub fn message_type(&self) -> u8 {
        match self {
            ItchMessage::AddOrder(_) => b'A',
            ItchMessage::AddOrderAttributed(_) => b'F',
            ItchMessage::OrderExecuted(_) => b'E',
            ItchMessage::OrderExecutedWithPrice(_) => b'C',
            ItchMessage::OrderCancel(_) => b'X',
            ItchMessage::OrderDelete(_) => b'D',
            ItchMessage::OrderReplace(_) => b'U',
        }
    }

    /// Numeric symbol identifier. Note that only 'A' and 'F' carry the ASCII
    /// ticker — every other message identifies its instrument by locate alone,
    /// so a receiver has to build the locate → ticker map from the add stream
    /// (or, on a real feed, from the Stock Directory messages).
    pub fn stock_locate(&self) -> u16 {
        match self {
            ItchMessage::AddOrder(m) => m.stock_locate,
            ItchMessage::AddOrderAttributed(m) => m.stock_locate,
            ItchMessage::OrderExecuted(m) => m.stock_locate,
            ItchMessage::OrderExecutedWithPrice(m) => m.stock_locate,
            ItchMessage::OrderCancel(m) => m.stock_locate,
            ItchMessage::OrderDelete(m) => m.stock_locate,
            ItchMessage::OrderReplace(m) => m.stock_locate,
        }
    }

    /// Nanoseconds since midnight, unpacked from the 48-bit field.
    pub fn timestamp_nanos(&self) -> u64 {
        let bytes = match self {
            ItchMessage::AddOrder(m) => m.timestamp_bytes,
            ItchMessage::AddOrderAttributed(m) => m.timestamp_bytes,
            ItchMessage::OrderExecuted(m) => m.timestamp_bytes,
            ItchMessage::OrderExecutedWithPrice(m) => m.timestamp_bytes,
            ItchMessage::OrderCancel(m) => m.timestamp_bytes,
            ItchMessage::OrderDelete(m) => m.timestamp_bytes,
            ItchMessage::OrderReplace(m) => m.timestamp_bytes,
        };
        unpack_itch_timestamp(&bytes)
    }

    /// Encoded size in bytes — always the fixed size for this message type.
    pub fn wire_len(&self) -> usize {
        crate::domain::codec::wire_len(self.message_type()).expect("every variant has a wire length")
    }
}
