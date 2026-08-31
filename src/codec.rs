//! Wire codec for ITCH 5.0 messages.
//!
//! Every field is written explicitly, big-endian, at a fixed offset. The
//! `#[repr(C, packed)]` layout of the structs in [`crate::models`] is *not* the
//! wire format and is never memcpy'd — that is how endianness bugs get in.

use std::fmt;

use crate::model::{unpack_itch_timestamp, ItchAddOrder};

/// Wire size of an Add Order ('A') message.
pub const ADD_ORDER_LEN: usize = 36;

/// Message type byte for Add Order (Anonymous).
pub const MSG_TYPE_ADD_ORDER: u8 = b'A';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// Output buffer is smaller than the encoded message.
    BufferTooSmall { need: usize, got: usize },
    /// Input is not exactly one message long.
    WrongLength { expected: usize, got: usize },
    /// Leading byte is not a message type we can decode.
    UnknownMessageType(u8),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::BufferTooSmall { need, got } => {
                write!(f, "output buffer too small: need {need} bytes, got {got}")
            }
            CodecError::WrongLength { expected, got } => {
                write!(f, "wrong payload length: expected {expected} bytes, got {got}")
            }
            CodecError::UnknownMessageType(b) => {
                write!(f, "unknown message type: 0x{b:02X} ({:?})", *b as char)
            }
        }
    }
}

impl std::error::Error for CodecError {}

/// Encodes an Add Order into `out`, returning the number of bytes written.
///
/// Field offsets (NASDAQ TotalView-ITCH 5.0, message type 'A'):
///
/// ```text
///  0      message_type        u8
///  1..3   stock_locate        u16 BE
///  3..5   tracking_number     u16 BE
///  5..11  timestamp           u48 BE
/// 11..19  order_reference     u64 BE
/// 19      buy_sell_indicator  u8
/// 20..24  shares              u32 BE
/// 24..32  stock               [u8; 8], space-padded
/// 32..36  price               u32 BE, scaled by 10,000
/// ```
pub fn encode_add_order(msg: &ItchAddOrder, out: &mut [u8]) -> Result<usize, CodecError> {
    if out.len() < ADD_ORDER_LEN {
        return Err(CodecError::BufferTooSmall {
            need: ADD_ORDER_LEN,
            got: out.len(),
        });
    }

    // Copy each field out of the packed struct before touching it; taking a
    // reference into a packed struct is a hard error (E0793).
    let stock_locate = msg.stock_locate;
    let tracking_number = msg.tracking_number;
    let timestamp_bytes = msg.timestamp_bytes;
    let order_reference = msg.order_reference;
    let shares = msg.shares;
    let stock = msg.stock;
    let price = msg.price;

    out[0] = msg.message_type as u8;
    out[1..3].copy_from_slice(&stock_locate.to_be_bytes());
    out[3..5].copy_from_slice(&tracking_number.to_be_bytes());
    out[5..11].copy_from_slice(&timestamp_bytes);
    out[11..19].copy_from_slice(&order_reference.to_be_bytes());
    out[19] = msg.buy_sell_indicator as u8;
    out[20..24].copy_from_slice(&shares.to_be_bytes());
    out[24..32].copy_from_slice(&stock);
    out[32..36].copy_from_slice(&price.to_be_bytes());

    Ok(ADD_ORDER_LEN)
}

/// Decodes exactly one Add Order from `src`.
///
/// `src` must be *exactly* 36 bytes. A datagram carrying one message and
/// nothing else should decode to one message and nothing else; trailing bytes
/// mean the sender and receiver disagree about the format, so say so loudly
/// rather than ignoring them.
pub fn decode_add_order(src: &[u8]) -> Result<ItchAddOrder, CodecError> {
    if src.len() != ADD_ORDER_LEN {
        return Err(CodecError::WrongLength {
            expected: ADD_ORDER_LEN,
            got: src.len(),
        });
    }
    if src[0] != MSG_TYPE_ADD_ORDER {
        return Err(CodecError::UnknownMessageType(src[0]));
    }

    Ok(ItchAddOrder {
        message_type: src[0] as _,
        stock_locate: u16::from_be_bytes([src[1], src[2]]),
        tracking_number: u16::from_be_bytes([src[3], src[4]]),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: u64::from_be_bytes(src[11..19].try_into().unwrap()),
        buy_sell_indicator: src[19] as _,
        shares: u32::from_be_bytes(src[20..24].try_into().unwrap()),
        stock: src[24..32].try_into().unwrap(),
        price: u32::from_be_bytes(src[32..36].try_into().unwrap()),
    })
}

/// Convenience: the decoded timestamp as nanoseconds since midnight.
pub fn timestamp_nanos(msg: &ItchAddOrder) -> u64 {
    let bytes = msg.timestamp_bytes;
    unpack_itch_timestamp(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{pack_itch_timestamp, pack_stock_symbol, unpack_stock_symbol};

    /// The hand-computed ground truth from `docs/1_SLICE.md`.
    ///
    /// Neither the encoder nor the decoder is allowed to define correctness —
    /// this table does. A round-trip test alone would pass even if the whole
    /// wire format were byte-reversed.
    const GOLDEN: [u8; ADD_ORDER_LEN] = [
        0x41, // 'A'          message_type
        0x00, 0x07, //        stock_locate    = 7
        0x00, 0x2A, //        tracking_number = 42
        0x1F, 0x1A, 0xCE, 0xD9, 0xF0, 0x00, // timestamp = 34_200_000_000_000 (09:30:00)
        0x00, 0x00, 0x00, 0x00, 0x49, 0x96, 0x02, 0xD2, // order_reference = 1_234_567_890
        0x42, // 'B'          buy_sell_indicator
        0x00, 0x00, 0x00, 0x64, // shares     = 100
        0x41, 0x41, 0x50, 0x4C, 0x20, 0x20, 0x20, 0x20, // stock = "AAPL    "
        0x00, 0x16, 0xED, 0x24, // price      = 1_502_500 ($150.25)
    ];

    fn golden_message() -> ItchAddOrder {
        ItchAddOrder {
            message_type: b'A' as _,
            stock_locate: 7,
            tracking_number: 42,
            timestamp_bytes: pack_itch_timestamp(34_200_000_000_000).unwrap(),
            order_reference: 1_234_567_890,
            buy_sell_indicator: b'B' as _,
            shares: 100,
            stock: pack_stock_symbol("AAPL").unwrap(),
            price: 1_502_500,
        }
    }

    #[test]
    fn encodes_to_the_golden_vector() {
        let mut buf = [0u8; ADD_ORDER_LEN];
        let n = encode_add_order(&golden_message(), &mut buf).unwrap();
        assert_eq!(n, ADD_ORDER_LEN);
        assert_eq!(buf, GOLDEN, "\n  got: {}\n  want: {}", crate::hex(&buf), crate::hex(&GOLDEN));
    }

    #[test]
    fn decodes_the_golden_vector() {
        let msg = decode_add_order(&GOLDEN).unwrap();
        assert_eq!(msg, golden_message());
        assert_eq!(timestamp_nanos(&msg), 34_200_000_000_000);
        assert_eq!(unpack_stock_symbol(&{ msg.stock }), "AAPL");
        assert_eq!(crate::format_price({ msg.price }), "150.2500");
    }

    #[test]
    fn round_trips() {
        let original = golden_message();
        let mut buf = [0u8; ADD_ORDER_LEN];
        let n = encode_add_order(&original, &mut buf).unwrap();
        assert_eq!(decode_add_order(&buf[..n]).unwrap(), original);
    }

    /// The u48 trap: the low six bytes are the *last* six, not the first six.
    #[test]
    fn timestamp_takes_the_low_six_bytes() {
        let packed = pack_itch_timestamp(34_200_000_000_000).unwrap();
        assert_eq!(packed, [0x1F, 0x1A, 0xCE, 0xD9, 0xF0, 0x00]);
        // Slicing [0..6] instead would give this, off by a factor of 65,536:
        assert_ne!(packed, [0x00, 0x00, 0x1F, 0x1A, 0xCE, 0xD9]);
        assert_eq!(unpack_itch_timestamp(&packed), 34_200_000_000_000);
    }

    #[test]
    fn timestamp_rejects_values_over_48_bits() {
        assert_eq!(pack_itch_timestamp((1 << 48) - 1), Some([0xFF; 6]));
        assert_eq!(pack_itch_timestamp(1 << 48), None);
        // A nanos-since-epoch value handed in by mistake must fail, not truncate.
        assert_eq!(pack_itch_timestamp(1_756_600_000_000_000_000), None);
    }

    /// Symbols are space-padded (0x20). NUL padding is the instinctive choice
    /// and it is wrong.
    #[test]
    fn symbols_are_space_padded() {
        assert_eq!(pack_stock_symbol("AAPL").unwrap(), *b"AAPL    ");
        assert_ne!(pack_stock_symbol("AAPL").unwrap(), [0x41, 0x41, 0x50, 0x4C, 0, 0, 0, 0]);
        assert_eq!(pack_stock_symbol("GOOGLE12").unwrap(), *b"GOOGLE12");
        assert_eq!(pack_stock_symbol("TOOLONG12"), None);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            decode_add_order(&GOLDEN[..35]),
            Err(CodecError::WrongLength { expected: 36, got: 35 })
        );
        // A 2048-byte buffer passed as `&buf` instead of `&buf[..n]`.
        let mut padded = [0u8; 2048];
        padded[..ADD_ORDER_LEN].copy_from_slice(&GOLDEN);
        assert!(matches!(decode_add_order(&padded), Err(CodecError::WrongLength { .. })));

        let mut wrong_type = GOLDEN;
        wrong_type[0] = b'D';
        assert_eq!(
            decode_add_order(&wrong_type),
            Err(CodecError::UnknownMessageType(b'D'))
        );

        let mut small = [0u8; 35];
        assert_eq!(
            encode_add_order(&golden_message(), &mut small),
            Err(CodecError::BufferTooSmall { need: 36, got: 35 })
        );
    }
}
