//! Wire codec for ITCH 5.0 messages.
//!
//! Every field is written explicitly, big-endian, at a fixed offset. The
//! `#[repr(C, packed)]` layout of the structs in [`crate::model`] is *not* the
//! wire format and is never memcpy'd — that is how endianness bugs get in.
//!
//! Every ITCH message shares an 11-byte prefix:
//!
//! ```text
//!  0      message_type     u8
//!  1..3   stock_locate     u16 BE
//!  3..5   tracking_number  u16 BE
//!  5..11  timestamp        u48 BE  (nanoseconds since midnight)
//! ```
//!
//! and the type byte alone determines the total length. That property is what
//! lets a receiver frame a stream without a length prefix — and it is exactly
//! the property that stops holding the moment either side learns a message type
//! the other doesn't. See `docs/session-layer.md`.

use std::fmt;

use crate::model::{
    unpack_itch_timestamp, ItchAddOrder, ItchAddOrderAttributed, ItchMessage, ItchOrderCancel,
    ItchOrderDelete, ItchOrderExecuted, ItchOrderExecutedWithPrice, ItchOrderReplace,
};

/// Bytes shared by every message type before the type-specific body.
pub const HEADER_LEN: usize = 11;

/// Wire size of an Add Order ('A') message.
pub const ADD_ORDER_LEN: usize = 36;
/// Wire size of an Add Order — Attributed ('F') message.
pub const ADD_ORDER_ATTRIBUTED_LEN: usize = 40;
/// Wire size of an Order Executed ('E') message.
pub const ORDER_EXECUTED_LEN: usize = 31;
/// Wire size of an Order Executed With Price ('C') message.
pub const ORDER_EXECUTED_WITH_PRICE_LEN: usize = 36;
/// Wire size of an Order Cancel ('X') message.
pub const ORDER_CANCEL_LEN: usize = 23;
/// Wire size of an Order Delete ('D') message.
pub const ORDER_DELETE_LEN: usize = 19;
/// Wire size of an Order Replace ('U') message.
pub const ORDER_REPLACE_LEN: usize = 35;

/// The largest message we emit. A send buffer this size fits anything.
pub const MAX_MESSAGE_LEN: usize = ADD_ORDER_ATTRIBUTED_LEN;

pub const MSG_TYPE_ADD_ORDER: u8 = b'A';
pub const MSG_TYPE_ADD_ORDER_ATTRIBUTED: u8 = b'F';
pub const MSG_TYPE_ORDER_EXECUTED: u8 = b'E';
pub const MSG_TYPE_ORDER_EXECUTED_WITH_PRICE: u8 = b'C';
pub const MSG_TYPE_ORDER_CANCEL: u8 = b'X';
pub const MSG_TYPE_ORDER_DELETE: u8 = b'D';
pub const MSG_TYPE_ORDER_REPLACE: u8 = b'U';

/// Encoded length for a message type byte, or `None` if we don't know the type.
///
/// `None` is the interesting case: a receiver that hits it cannot skip the
/// message, because it has no idea how long the message is. On a framed stream
/// that desynchronizes everything after it.
pub const fn wire_len(type_byte: u8) -> Option<usize> {
    match type_byte {
        MSG_TYPE_ADD_ORDER => Some(ADD_ORDER_LEN),
        MSG_TYPE_ADD_ORDER_ATTRIBUTED => Some(ADD_ORDER_ATTRIBUTED_LEN),
        MSG_TYPE_ORDER_EXECUTED => Some(ORDER_EXECUTED_LEN),
        MSG_TYPE_ORDER_EXECUTED_WITH_PRICE => Some(ORDER_EXECUTED_WITH_PRICE_LEN),
        MSG_TYPE_ORDER_CANCEL => Some(ORDER_CANCEL_LEN),
        MSG_TYPE_ORDER_DELETE => Some(ORDER_DELETE_LEN),
        MSG_TYPE_ORDER_REPLACE => Some(ORDER_REPLACE_LEN),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// Output buffer is smaller than the encoded message.
    BufferTooSmall { need: usize, got: usize },
    /// Input is not exactly one message long.
    WrongLength { expected: usize, got: usize },
    /// Input is too short to even read the type byte.
    Empty,
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
            CodecError::Empty => write!(f, "empty payload: no message type byte"),
            CodecError::UnknownMessageType(b) => {
                write!(f, "unknown message type: 0x{b:02X} ({:?})", *b as char)
            }
        }
    }
}

impl std::error::Error for CodecError {}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Encodes any ITCH message into `out`, returning the number of bytes written.
pub fn encode(msg: &ItchMessage, out: &mut [u8]) -> Result<usize, CodecError> {
    match msg {
        ItchMessage::AddOrder(m) => encode_add_order(m, out),
        ItchMessage::AddOrderAttributed(m) => encode_add_order_attributed(m, out),
        ItchMessage::OrderExecuted(m) => encode_order_executed(m, out),
        ItchMessage::OrderExecutedWithPrice(m) => encode_order_executed_with_price(m, out),
        ItchMessage::OrderCancel(m) => encode_order_cancel(m, out),
        ItchMessage::OrderDelete(m) => encode_order_delete(m, out),
        ItchMessage::OrderReplace(m) => encode_order_replace(m, out),
    }
}

/// Decodes exactly one ITCH message from `src`.
///
/// `src` must be *exactly* the length its type byte implies. A datagram
/// carrying one message and nothing else should decode to one message and
/// nothing else; trailing bytes mean the sender and receiver disagree about the
/// format, so say so loudly rather than ignoring them.
pub fn decode(src: &[u8]) -> Result<ItchMessage, CodecError> {
    let &type_byte = src.first().ok_or(CodecError::Empty)?;
    let expected = wire_len(type_byte).ok_or(CodecError::UnknownMessageType(type_byte))?;
    if src.len() != expected {
        return Err(CodecError::WrongLength { expected, got: src.len() });
    }
    Ok(match type_byte {
        MSG_TYPE_ADD_ORDER => ItchMessage::AddOrder(decode_add_order(src)?),
        MSG_TYPE_ADD_ORDER_ATTRIBUTED => {
            ItchMessage::AddOrderAttributed(decode_add_order_attributed(src)?)
        }
        MSG_TYPE_ORDER_EXECUTED => ItchMessage::OrderExecuted(decode_order_executed(src)?),
        MSG_TYPE_ORDER_EXECUTED_WITH_PRICE => {
            ItchMessage::OrderExecutedWithPrice(decode_order_executed_with_price(src)?)
        }
        MSG_TYPE_ORDER_CANCEL => ItchMessage::OrderCancel(decode_order_cancel(src)?),
        MSG_TYPE_ORDER_DELETE => ItchMessage::OrderDelete(decode_order_delete(src)?),
        MSG_TYPE_ORDER_REPLACE => ItchMessage::OrderReplace(decode_order_replace(src)?),
        _ => unreachable!("wire_len already rejected unknown types"),
    })
}

// ---------------------------------------------------------------------------
// Shared 11-byte prefix
// ---------------------------------------------------------------------------

fn check_out(out: &[u8], need: usize) -> Result<(), CodecError> {
    if out.len() < need {
        Err(CodecError::BufferTooSmall { need, got: out.len() })
    } else {
        Ok(())
    }
}

fn check_in(src: &[u8], expected: usize, type_byte: u8) -> Result<(), CodecError> {
    if src.len() != expected {
        return Err(CodecError::WrongLength { expected, got: src.len() });
    }
    if src[0] != type_byte {
        return Err(CodecError::UnknownMessageType(src[0]));
    }
    Ok(())
}

#[inline]
fn put_header(out: &mut [u8], type_byte: u8, locate: u16, tracking: u16, ts: [u8; 6]) {
    out[0] = type_byte;
    out[1..3].copy_from_slice(&locate.to_be_bytes());
    out[3..5].copy_from_slice(&tracking.to_be_bytes());
    out[5..11].copy_from_slice(&ts);
}

#[inline]
fn get_u16(src: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([src[at], src[at + 1]])
}

#[inline]
fn get_u32(src: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(src[at..at + 4].try_into().unwrap())
}

#[inline]
fn get_u64(src: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(src[at..at + 8].try_into().unwrap())
}

// ---------------------------------------------------------------------------
// 'A' — Add Order
// ---------------------------------------------------------------------------

/// ```text
/// 11..19  order_reference     u64 BE
/// 19      buy_sell_indicator  u8
/// 20..24  shares              u32 BE
/// 24..32  stock               [u8; 8], space-padded
/// 32..36  price               u32 BE, scaled by 10,000
/// ```
pub fn encode_add_order(msg: &ItchAddOrder, out: &mut [u8]) -> Result<usize, CodecError> {
    check_out(out, ADD_ORDER_LEN)?;
    // Copy each field out of the packed struct before touching it; taking a
    // reference into a packed struct is a hard error (E0793).
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let (order_ref, shares, stock, price) =
        (msg.order_reference, msg.shares, msg.stock, msg.price);

    put_header(out, MSG_TYPE_ADD_ORDER, locate, tracking, ts);
    out[11..19].copy_from_slice(&order_ref.to_be_bytes());
    out[19] = msg.buy_sell_indicator;
    out[20..24].copy_from_slice(&shares.to_be_bytes());
    out[24..32].copy_from_slice(&stock);
    out[32..36].copy_from_slice(&price.to_be_bytes());
    Ok(ADD_ORDER_LEN)
}

pub fn decode_add_order(src: &[u8]) -> Result<ItchAddOrder, CodecError> {
    check_in(src, ADD_ORDER_LEN, MSG_TYPE_ADD_ORDER)?;
    Ok(ItchAddOrder {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: get_u64(src, 11),
        buy_sell_indicator: src[19],
        shares: get_u32(src, 20),
        stock: src[24..32].try_into().unwrap(),
        price: get_u32(src, 32),
    })
}

// ---------------------------------------------------------------------------
// 'F' — Add Order, Attributed
// ---------------------------------------------------------------------------

/// Identical to 'A' through byte 36, then a 4-byte MPID.
pub fn encode_add_order_attributed(
    msg: &ItchAddOrderAttributed,
    out: &mut [u8],
) -> Result<usize, CodecError> {
    check_out(out, ADD_ORDER_ATTRIBUTED_LEN)?;
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let (order_ref, shares, stock, price, attribution) =
        (msg.order_reference, msg.shares, msg.stock, msg.price, msg.attribution);

    put_header(out, MSG_TYPE_ADD_ORDER_ATTRIBUTED, locate, tracking, ts);
    out[11..19].copy_from_slice(&order_ref.to_be_bytes());
    out[19] = msg.buy_sell_indicator;
    out[20..24].copy_from_slice(&shares.to_be_bytes());
    out[24..32].copy_from_slice(&stock);
    out[32..36].copy_from_slice(&price.to_be_bytes());
    out[36..40].copy_from_slice(&attribution);
    Ok(ADD_ORDER_ATTRIBUTED_LEN)
}

pub fn decode_add_order_attributed(src: &[u8]) -> Result<ItchAddOrderAttributed, CodecError> {
    check_in(src, ADD_ORDER_ATTRIBUTED_LEN, MSG_TYPE_ADD_ORDER_ATTRIBUTED)?;
    Ok(ItchAddOrderAttributed {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: get_u64(src, 11),
        buy_sell_indicator: src[19],
        shares: get_u32(src, 20),
        stock: src[24..32].try_into().unwrap(),
        price: get_u32(src, 32),
        attribution: src[36..40].try_into().unwrap(),
    })
}

// ---------------------------------------------------------------------------
// 'E' — Order Executed
// ---------------------------------------------------------------------------

/// ```text
/// 11..19  order_reference  u64 BE
/// 19..23  shares           u32 BE  (executed quantity)
/// 23..31  match_number     u64 BE
/// ```
pub fn encode_order_executed(msg: &ItchOrderExecuted, out: &mut [u8]) -> Result<usize, CodecError> {
    check_out(out, ORDER_EXECUTED_LEN)?;
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let (order_ref, shares, match_number) =
        (msg.order_reference, msg.shares, msg.match_number);

    put_header(out, MSG_TYPE_ORDER_EXECUTED, locate, tracking, ts);
    out[11..19].copy_from_slice(&order_ref.to_be_bytes());
    out[19..23].copy_from_slice(&shares.to_be_bytes());
    out[23..31].copy_from_slice(&match_number.to_be_bytes());
    Ok(ORDER_EXECUTED_LEN)
}

pub fn decode_order_executed(src: &[u8]) -> Result<ItchOrderExecuted, CodecError> {
    check_in(src, ORDER_EXECUTED_LEN, MSG_TYPE_ORDER_EXECUTED)?;
    Ok(ItchOrderExecuted {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: get_u64(src, 11),
        shares: get_u32(src, 19),
        match_number: get_u64(src, 23),
    })
}

// ---------------------------------------------------------------------------
// 'C' — Order Executed With Price
// ---------------------------------------------------------------------------

/// 'E' plus a printable flag and the off-book execution price.
pub fn encode_order_executed_with_price(
    msg: &ItchOrderExecutedWithPrice,
    out: &mut [u8],
) -> Result<usize, CodecError> {
    check_out(out, ORDER_EXECUTED_WITH_PRICE_LEN)?;
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let (order_ref, shares, match_number, execution_price) =
        (msg.order_reference, msg.shares, msg.match_number, msg.execution_price);

    put_header(out, MSG_TYPE_ORDER_EXECUTED_WITH_PRICE, locate, tracking, ts);
    out[11..19].copy_from_slice(&order_ref.to_be_bytes());
    out[19..23].copy_from_slice(&shares.to_be_bytes());
    out[23..31].copy_from_slice(&match_number.to_be_bytes());
    out[31] = msg.printable;
    out[32..36].copy_from_slice(&execution_price.to_be_bytes());
    Ok(ORDER_EXECUTED_WITH_PRICE_LEN)
}

pub fn decode_order_executed_with_price(
    src: &[u8],
) -> Result<ItchOrderExecutedWithPrice, CodecError> {
    check_in(src, ORDER_EXECUTED_WITH_PRICE_LEN, MSG_TYPE_ORDER_EXECUTED_WITH_PRICE)?;
    Ok(ItchOrderExecutedWithPrice {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: get_u64(src, 11),
        shares: get_u32(src, 19),
        match_number: get_u64(src, 23),
        printable: src[31],
        execution_price: get_u32(src, 32),
    })
}

// ---------------------------------------------------------------------------
// 'X' — Order Cancel
// ---------------------------------------------------------------------------

/// ```text
/// 11..19  order_reference  u64 BE
/// 19..23  canceled_shares  u32 BE  (partial — the order stays live)
/// ```
pub fn encode_order_cancel(msg: &ItchOrderCancel, out: &mut [u8]) -> Result<usize, CodecError> {
    check_out(out, ORDER_CANCEL_LEN)?;
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let (order_ref, canceled) = (msg.order_reference, msg.canceled_shares);

    put_header(out, MSG_TYPE_ORDER_CANCEL, locate, tracking, ts);
    out[11..19].copy_from_slice(&order_ref.to_be_bytes());
    out[19..23].copy_from_slice(&canceled.to_be_bytes());
    Ok(ORDER_CANCEL_LEN)
}

pub fn decode_order_cancel(src: &[u8]) -> Result<ItchOrderCancel, CodecError> {
    check_in(src, ORDER_CANCEL_LEN, MSG_TYPE_ORDER_CANCEL)?;
    Ok(ItchOrderCancel {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: get_u64(src, 11),
        canceled_shares: get_u32(src, 19),
    })
}

// ---------------------------------------------------------------------------
// 'D' — Order Delete
// ---------------------------------------------------------------------------

/// The smallest ITCH message: header plus the reference to drop.
pub fn encode_order_delete(msg: &ItchOrderDelete, out: &mut [u8]) -> Result<usize, CodecError> {
    check_out(out, ORDER_DELETE_LEN)?;
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let order_ref = msg.order_reference;

    put_header(out, MSG_TYPE_ORDER_DELETE, locate, tracking, ts);
    out[11..19].copy_from_slice(&order_ref.to_be_bytes());
    Ok(ORDER_DELETE_LEN)
}

pub fn decode_order_delete(src: &[u8]) -> Result<ItchOrderDelete, CodecError> {
    check_in(src, ORDER_DELETE_LEN, MSG_TYPE_ORDER_DELETE)?;
    Ok(ItchOrderDelete {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        order_reference: get_u64(src, 11),
    })
}

// ---------------------------------------------------------------------------
// 'U' — Order Replace
// ---------------------------------------------------------------------------

/// ```text
/// 11..19  original_order_reference  u64 BE  (dies here)
/// 19..27  new_order_reference       u64 BE  (born here)
/// 27..31  shares                    u32 BE
/// 31..35  price                     u32 BE
/// ```
///
/// Replace is a delete and an add fused into one message — a receiver that
/// treats it as an in-place edit will leak the old reference.
pub fn encode_order_replace(msg: &ItchOrderReplace, out: &mut [u8]) -> Result<usize, CodecError> {
    check_out(out, ORDER_REPLACE_LEN)?;
    let (locate, tracking, ts) = (msg.stock_locate, msg.tracking_number, msg.timestamp_bytes);
    let (old_ref, new_ref, shares, price) = (
        msg.original_order_reference,
        msg.new_order_reference,
        msg.shares,
        msg.price,
    );

    put_header(out, MSG_TYPE_ORDER_REPLACE, locate, tracking, ts);
    out[11..19].copy_from_slice(&old_ref.to_be_bytes());
    out[19..27].copy_from_slice(&new_ref.to_be_bytes());
    out[27..31].copy_from_slice(&shares.to_be_bytes());
    out[31..35].copy_from_slice(&price.to_be_bytes());
    Ok(ORDER_REPLACE_LEN)
}

pub fn decode_order_replace(src: &[u8]) -> Result<ItchOrderReplace, CodecError> {
    check_in(src, ORDER_REPLACE_LEN, MSG_TYPE_ORDER_REPLACE)?;
    Ok(ItchOrderReplace {
        message_type: src[0],
        stock_locate: get_u16(src, 1),
        tracking_number: get_u16(src, 3),
        timestamp_bytes: src[5..11].try_into().unwrap(),
        original_order_reference: get_u64(src, 11),
        new_order_reference: get_u64(src, 19),
        shares: get_u32(src, 27),
        price: get_u32(src, 31),
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
            message_type: b'A',
            stock_locate: 7,
            tracking_number: 42,
            timestamp_bytes: pack_itch_timestamp(34_200_000_000_000).unwrap(),
            order_reference: 1_234_567_890,
            buy_sell_indicator: b'B',
            shares: 100,
            stock: pack_stock_symbol("AAPL").unwrap(),
            price: 1_502_500,
        }
    }

    /// One of each variant, with every field set to a distinct value so a
    /// transposed pair of offsets cannot survive the round trip.
    fn sample_messages() -> Vec<ItchMessage> {
        let ts = pack_itch_timestamp(34_200_000_000_000).unwrap();
        vec![
            ItchMessage::AddOrder(golden_message()),
            ItchMessage::AddOrderAttributed(ItchAddOrderAttributed {
                message_type: b'F',
                stock_locate: 11,
                tracking_number: 22,
                timestamp_bytes: ts,
                order_reference: 0x0102_0304_0506_0708,
                buy_sell_indicator: b'S',
                shares: 300,
                stock: pack_stock_symbol("MSFT").unwrap(),
                price: 3_801_200,
                attribution: *b"NSDQ",
            }),
            ItchMessage::OrderExecuted(ItchOrderExecuted {
                message_type: b'E',
                stock_locate: 3,
                tracking_number: 4,
                timestamp_bytes: ts,
                order_reference: 999_999,
                shares: 250,
                match_number: 0x1122_3344_5566_7788,
            }),
            ItchMessage::OrderExecutedWithPrice(ItchOrderExecutedWithPrice {
                message_type: b'C',
                stock_locate: 5,
                tracking_number: 6,
                timestamp_bytes: ts,
                order_reference: 12_345,
                shares: 75,
                match_number: 67_890,
                printable: b'N',
                execution_price: 1_499_900,
            }),
            ItchMessage::OrderCancel(ItchOrderCancel {
                message_type: b'X',
                stock_locate: 8,
                tracking_number: 9,
                timestamp_bytes: ts,
                order_reference: 555,
                canceled_shares: 40,
            }),
            ItchMessage::OrderDelete(ItchOrderDelete {
                message_type: b'D',
                stock_locate: 12,
                tracking_number: 13,
                timestamp_bytes: ts,
                order_reference: 777,
            }),
            ItchMessage::OrderReplace(ItchOrderReplace {
                message_type: b'U',
                stock_locate: 14,
                tracking_number: 15,
                timestamp_bytes: ts,
                original_order_reference: 1_000,
                new_order_reference: 2_000,
                shares: 500,
                price: 2_502_500,
            }),
        ]
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
        assert_eq!(crate::format_price(msg.price), "150.2500");
    }

    #[test]
    fn round_trips() {
        let original = golden_message();
        let mut buf = [0u8; ADD_ORDER_LEN];
        let n = encode_add_order(&original, &mut buf).unwrap();
        assert_eq!(decode_add_order(&buf[..n]).unwrap(), original);
    }

    #[test]
    fn every_variant_round_trips_through_dispatch() {
        for msg in sample_messages() {
            let mut buf = [0u8; MAX_MESSAGE_LEN];
            let n = encode(&msg, &mut buf).unwrap();
            assert_eq!(n, msg.wire_len(), "declared length disagrees with encoder");
            assert_eq!(n, wire_len(msg.message_type()).unwrap());
            assert_eq!(buf[0], msg.message_type(), "type byte must lead");
            let back = decode(&buf[..n]).unwrap();
            assert_eq!(back, msg);
            assert_eq!(back.stock_locate(), msg.stock_locate());
            assert_eq!(back.timestamp_nanos(), 34_200_000_000_000);
        }
    }

    /// The lengths are the spec's, not ours. Hard-coded here so a "cleanup"
    /// that renumbers an offset has to change this table too.
    #[test]
    fn wire_lengths_match_the_spec() {
        assert_eq!(wire_len(b'A'), Some(36));
        assert_eq!(wire_len(b'F'), Some(40));
        assert_eq!(wire_len(b'E'), Some(31));
        assert_eq!(wire_len(b'C'), Some(36));
        assert_eq!(wire_len(b'X'), Some(23));
        assert_eq!(wire_len(b'D'), Some(19));
        assert_eq!(wire_len(b'U'), Some(35));
        assert_eq!(wire_len(b'P'), None, "unknown types must be unknown, not guessed");
        // Every length starts with the shared header.
        for t in [b'A', b'F', b'E', b'C', b'X', b'D', b'U'] {
            assert!(wire_len(t).unwrap() >= HEADER_LEN);
            assert!(wire_len(t).unwrap() <= MAX_MESSAGE_LEN);
        }
    }

    /// 'A' and 'C' are both 36 bytes. Length alone cannot identify a message;
    /// only the type byte can.
    #[test]
    fn same_length_different_types_do_not_collide() {
        let mut a = [0u8; MAX_MESSAGE_LEN];
        let mut c = [0u8; MAX_MESSAGE_LEN];
        let msgs = sample_messages();
        let na = encode(&msgs[0], &mut a).unwrap();
        let nc = encode(&msgs[3], &mut c).unwrap();
        assert_eq!(na, nc);
        assert_ne!(a[..na], c[..nc]);
        assert!(matches!(decode(&a[..na]).unwrap(), ItchMessage::AddOrder(_)));
        assert!(matches!(
            decode(&c[..nc]).unwrap(),
            ItchMessage::OrderExecutedWithPrice(_)
        ));
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
        assert!(matches!(decode(&padded), Err(CodecError::WrongLength { .. })));

        let mut wrong_type = GOLDEN;
        wrong_type[0] = b'D';
        assert_eq!(decode_add_order(&wrong_type), Err(CodecError::UnknownMessageType(b'D')));
        // Through dispatch, 'D' is a *known* type — it is the length that is wrong.
        assert_eq!(
            decode(&wrong_type),
            Err(CodecError::WrongLength { expected: 19, got: 36 })
        );

        let mut unknown = GOLDEN;
        unknown[0] = b'P';
        assert_eq!(decode(&unknown), Err(CodecError::UnknownMessageType(b'P')));
        assert_eq!(decode(&[]), Err(CodecError::Empty));

        let mut small = [0u8; 35];
        assert_eq!(
            encode_add_order(&golden_message(), &mut small),
            Err(CodecError::BufferTooSmall { need: 36, got: 35 })
        );
    }
}
