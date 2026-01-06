//! Low-level protobuf wire format parsing.
//!
//! This module implements the protobuf wire format parsing needed to
//! correctly identify record boundaries in binary data.
//!
//! ## Wire Format Overview
//!
//! Each protobuf field is encoded as:
//! - A varint "tag" containing the field number and wire type
//! - The field data (format depends on wire type)
//!
//! Wire types:
//! - 0: VARINT (int32, int64, uint32, uint64, sint32, sint64, bool, enum)
//! - 1: I64 (fixed64, sfixed64, double)
//! - 2: LEN (string, bytes, embedded messages, packed repeated fields)
//! - 5: I32 (fixed32, sfixed32, float)

use crate::error::{Error, Result};

/// Protobuf wire types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WireType {
    /// Variable-length integer
    Varint = 0,
    /// 64-bit fixed-width
    I64 = 1,
    /// Length-delimited (strings, bytes, embedded messages)
    Len = 2,
    /// Start group (deprecated)
    StartGroup = 3,
    /// End group (deprecated)
    EndGroup = 4,
    /// 32-bit fixed-width
    I32 = 5,
}

impl TryFrom<u8> for WireType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(WireType::Varint),
            1 => Ok(WireType::I64),
            2 => Ok(WireType::Len),
            3 => Ok(WireType::StartGroup),
            4 => Ok(WireType::EndGroup),
            5 => Ok(WireType::I32),
            _ => Err(Error::invalid_wire_format(
                0,
                format!("unknown wire type: {}", value),
            )),
        }
    }
}

/// Maximum valid protobuf field number (2^29 - 1)
pub const MAX_VALID_NUMBER: u32 = 536_870_911;

/// Decode a varint from the given bytes.
///
/// Returns the decoded value and the number of bytes consumed.
pub fn decode_varint(data: &[u8]) -> Result<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;

    for (i, &byte) in data.iter().enumerate() {
        if i >= 10 {
            // Varints are at most 10 bytes for a 64-bit value
            return Err(Error::varint_decode(i));
        }

        result |= ((byte & 0x7F) as u64) << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            return Ok((result, i + 1));
        }
    }

    Err(Error::varint_decode(data.len()))
}

/// Consume a single protobuf field from the data.
///
/// Returns the field number and total bytes consumed (including tag and value).
pub fn consume_field(data: &[u8]) -> Result<(u32, usize)> {
    if data.is_empty() {
        return Err(Error::invalid_wire_format(0, "empty data"));
    }

    // Decode the tag (varint containing field number and wire type)
    let (tag, tag_len) = decode_varint(data).map_err(|_| {
        Error::invalid_wire_format(0, "failed to decode field tag")
    })?;

    let wire_type = WireType::try_from((tag & 0x07) as u8)?;
    let field_number = (tag >> 3) as u32;

    // Validate field number
    if field_number == 0 || field_number > MAX_VALID_NUMBER {
        return Err(Error::InvalidFieldNumber {
            number: field_number,
            max: MAX_VALID_NUMBER,
        });
    }

    // Calculate bytes consumed based on wire type
    let value_len = match wire_type {
        WireType::Varint => {
            // Consume the varint value
            let remaining = &data[tag_len..];
            let (_, varint_len) = decode_varint(remaining).map_err(|_| {
                Error::invalid_wire_format(tag_len, "failed to decode varint value")
            })?;
            varint_len
        }
        WireType::I64 => {
            // Fixed 8 bytes
            if data.len() < tag_len + 8 {
                return Err(Error::invalid_wire_format(
                    tag_len,
                    "not enough bytes for I64",
                ));
            }
            8
        }
        WireType::Len => {
            // Length-prefixed: decode length varint, then skip that many bytes
            let remaining = &data[tag_len..];
            let (length, length_varint_len) = decode_varint(remaining).map_err(|_| {
                Error::invalid_wire_format(tag_len, "failed to decode length prefix")
            })?;

            let total_value_len = length_varint_len + length as usize;
            if data.len() < tag_len + total_value_len {
                return Err(Error::invalid_wire_format(
                    tag_len,
                    format!(
                        "not enough bytes for LEN field (need {}, have {})",
                        length,
                        data.len() - tag_len - length_varint_len
                    ),
                ));
            }
            total_value_len
        }
        WireType::StartGroup | WireType::EndGroup => {
            // Groups are deprecated and complex to parse
            // For our purposes, we can treat them as 0 additional bytes
            // (the tag itself is the marker)
            0
        }
        WireType::I32 => {
            // Fixed 4 bytes
            if data.len() < tag_len + 4 {
                return Err(Error::invalid_wire_format(
                    tag_len,
                    "not enough bytes for I32",
                ));
            }
            4
        }
    };

    Ok((field_number, tag_len + value_len))
}

/// Consume multiple fields and return total bytes consumed.
///
/// Stops when it runs out of data or encounters an error.
pub fn consume_fields(data: &[u8]) -> usize {
    let mut position = 0;

    while position < data.len() {
        match consume_field(&data[position..]) {
            Ok((_, len)) => {
                position += len;
            }
            Err(_) => break,
        }
    }

    position
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_varint_single_byte() {
        let data = [0x08]; // Value 8
        let (value, len) = decode_varint(&data).unwrap();
        assert_eq!(value, 8);
        assert_eq!(len, 1);
    }

    #[test]
    fn test_decode_varint_multi_byte() {
        let data = [0xAC, 0x02]; // Value 300
        let (value, len) = decode_varint(&data).unwrap();
        assert_eq!(value, 300);
        assert_eq!(len, 2);
    }

    #[test]
    fn test_decode_varint_max() {
        // Maximum 64-bit varint (all 1s)
        let data = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
        let (value, len) = decode_varint(&data).unwrap();
        assert_eq!(value, u64::MAX);
        assert_eq!(len, 10);
    }

    #[test]
    fn test_wire_type_conversion() {
        assert_eq!(WireType::try_from(0).unwrap(), WireType::Varint);
        assert_eq!(WireType::try_from(1).unwrap(), WireType::I64);
        assert_eq!(WireType::try_from(2).unwrap(), WireType::Len);
        assert_eq!(WireType::try_from(5).unwrap(), WireType::I32);
        assert!(WireType::try_from(6).is_err());
    }

    #[test]
    fn test_consume_varint_field() {
        // Field 1, wire type 0 (varint), value 150
        let data = [0x08, 0x96, 0x01];
        let (field_num, len) = consume_field(&data).unwrap();
        assert_eq!(field_num, 1);
        assert_eq!(len, 3);
    }

    #[test]
    fn test_consume_len_field() {
        // Field 1, wire type 2 (len), length 5, "hello"
        let data = [0x0A, 0x05, b'h', b'e', b'l', b'l', b'o'];
        let (field_num, len) = consume_field(&data).unwrap();
        assert_eq!(field_num, 1);
        assert_eq!(len, 7);
    }

    #[test]
    fn test_consume_fixed32_field() {
        // Field 1, wire type 5 (I32), 4 bytes
        let data = [0x0D, 0x01, 0x02, 0x03, 0x04];
        let (field_num, len) = consume_field(&data).unwrap();
        assert_eq!(field_num, 1);
        assert_eq!(len, 5);
    }

    #[test]
    fn test_consume_fixed64_field() {
        // Field 1, wire type 1 (I64), 8 bytes
        let data = [0x09, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let (field_num, len) = consume_field(&data).unwrap();
        assert_eq!(field_num, 1);
        assert_eq!(len, 9);
    }

    #[test]
    fn test_invalid_field_number() {
        // Field 0 is invalid
        let data = [0x00, 0x01];
        assert!(consume_field(&data).is_err());
    }
}
