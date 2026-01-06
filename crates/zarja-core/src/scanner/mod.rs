//! Binary scanning module for finding embedded protobuf descriptors.
//!
//! This module provides functionality to scan binary files for embedded
//! `FileDescriptorProto` data and extract it for reconstruction.
//!
//! ## Algorithm Overview
//!
//! 1. Search for the `.proto` byte sequence in the binary
//! 2. Backtrack to find the magic byte `0x0A` (field 1, wire type LEN)
//! 3. Parse forward using protobuf wire format to find record boundaries
//! 4. Extract the complete `FileDescriptorProto` bytes
//!
//! ## Extensibility
//!
//! The [`ScanStrategy`] trait allows custom scanning algorithms:
//!
//! ```no_run
//! use zarja_core::scanner::{ScanStrategy, ScanResult};
//! use zarja_core::Result;
//!
//! struct CustomScanner;
//!
//! impl ScanStrategy for CustomScanner {
//!     fn scan(&self, data: &[u8]) -> Result<Vec<ScanResult>> {
//!         // Custom scanning logic
//!         Ok(vec![])
//!     }
//! }
//! ```

mod wire;

use crate::error::{Error, Result};
use std::ops::Range;
use tracing::{debug, trace};

pub use wire::{WireType, decode_varint, consume_field, consume_fields, MAX_VALID_NUMBER};

/// Pattern to search for in binaries (filename suffix)
const PROTO_SUFFIX: &[u8] = b".proto";

/// Magic byte indicating start of FileDescriptorProto
/// This is field 1 (name) with wire type 2 (LEN): (1 << 3) | 2 = 0x0A
const MAGIC_BYTE: u8 = 0x0A;

/// Result of scanning a binary for a single descriptor
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// The raw bytes of the FileDescriptorProto
    pub data: Vec<u8>,
    /// Byte range in the original input where this was found
    pub range: Range<usize>,
}

impl ScanResult {
    /// Creates a new scan result
    pub fn new(data: Vec<u8>, range: Range<usize>) -> Self {
        Self { data, range }
    }

    /// Returns the data as a slice
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }
}

/// Configuration for the scanner
#[derive(Debug, Clone)]
pub struct ScannerConfig {
    /// Maximum number of descriptors to find (0 = unlimited)
    pub max_results: usize,
    /// Minimum size for a valid descriptor (filters noise)
    pub min_descriptor_size: usize,
    /// Maximum size for a valid descriptor (filters garbage)
    pub max_descriptor_size: usize,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            max_results: 0,
            min_descriptor_size: 10,
            max_descriptor_size: 10 * 1024 * 1024, // 10 MB
        }
    }
}

impl ScannerConfig {
    /// Creates a new scanner config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of results to return
    pub fn max_results(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Sets the minimum descriptor size filter
    pub fn min_descriptor_size(mut self, size: usize) -> Self {
        self.min_descriptor_size = size;
        self
    }

    /// Sets the maximum descriptor size filter
    pub fn max_descriptor_size(mut self, size: usize) -> Self {
        self.max_descriptor_size = size;
        self
    }
}

/// Trait for implementing custom scanning strategies
///
/// This trait allows you to plug in different algorithms for finding
/// protobuf descriptors in binary data.
pub trait ScanStrategy: Send + Sync {
    /// Scan the provided data for protobuf descriptors
    fn scan(&self, data: &[u8]) -> Result<Vec<ScanResult>>;

    /// Scan the data and return an iterator (for streaming large files)
    fn scan_iter<'a>(&'a self, data: &'a [u8]) -> Box<dyn Iterator<Item = Result<ScanResult>> + 'a> {
        // Default implementation: collect all results into a vec and iterate
        match self.scan(data) {
            Ok(results) => Box::new(results.into_iter().map(Ok)),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }
}

/// Primary scanner for finding embedded protobuf descriptors
#[derive(Debug, Clone)]
pub struct Scanner {
    config: ScannerConfig,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    /// Creates a new scanner with default configuration
    pub fn new() -> Self {
        Self {
            config: ScannerConfig::default(),
        }
    }

    /// Creates a new scanner with custom configuration
    pub fn with_config(config: ScannerConfig) -> Self {
        Self { config }
    }

    /// Consumes protobuf fields starting from the given position
    /// Returns the number of bytes consumed for the complete record
    fn consume_record(&self, data: &[u8], start: usize) -> Result<usize> {
        let mut position = start;
        let mut consumed_field_one = false;

        loop {
            if position >= data.len() {
                // Reached end of data, return what we have
                return Ok(position - start);
            }

            match consume_field(&data[position..]) {
                Ok((field_number, length)) => {
                    // If we see field 1 again, we've hit the next descriptor
                    // (adjacent descriptors in binary)
                    if field_number == 1 {
                        if consumed_field_one {
                            trace!(
                                "Found adjacent descriptor at position {}",
                                position
                            );
                            return Ok(position - start);
                        }
                        consumed_field_one = true;
                    }

                    position += length;

                    // Safety check: don't exceed data bounds
                    if position > data.len() {
                        return Ok(data.len() - start);
                    }
                }
                Err(_) => {
                    // Hit invalid data, return what we have so far
                    return Ok(position - start);
                }
            }
        }
    }

    /// Find the start of a FileDescriptorProto by backtracking from a `.proto` match
    fn find_record_start(&self, data: &[u8], proto_suffix_pos: usize) -> Option<usize> {
        // We need to backtrack to find the 0x0A byte that starts the record
        // The structure is: 0x0A [varint length] [filename bytes ending in .proto]

        // The .proto suffix is at proto_suffix_pos, so the filename ends at proto_suffix_pos + 6
        // We need to find where the filename starts

        // Search backwards for the magic byte
        let search_start = proto_suffix_pos.saturating_sub(256); // Filenames shouldn't be longer than 256 bytes

        for i in (search_start..proto_suffix_pos).rev() {
            if data[i] == MAGIC_BYTE {
                // Verify this is a valid length-prefixed string
                if i + 1 < data.len() {
                    // Try to decode the length varint
                    if let Ok((length, varint_len)) = decode_varint(&data[i + 1..]) {
                        let expected_end = i + 1 + varint_len + length as usize;
                        let actual_end = proto_suffix_pos + PROTO_SUFFIX.len();

                        // Check if this length matches our .proto position
                        if expected_end == actual_end {
                            return Some(i);
                        }

                        // Edge case: filename is exactly 10 chars, 0x0A might be the length
                        if length == 10 && i > 0 && data[i - 1] == MAGIC_BYTE {
                            return Some(i - 1);
                        }
                    }
                }
            }
        }

        None
    }
}

impl ScanStrategy for Scanner {
    fn scan(&self, data: &[u8]) -> Result<Vec<ScanResult>> {
        let mut results = Vec::new();
        let mut position = 0;

        debug!("Starting scan of {} bytes", data.len());

        while position < data.len() {
            // Find next occurrence of ".proto"
            let remaining = &data[position..];
            let proto_pos = find_subsequence(remaining, PROTO_SUFFIX);

            let Some(relative_pos) = proto_pos else {
                break;
            };

            let absolute_pos = position + relative_pos;
            trace!("Found .proto suffix at position {}", absolute_pos);

            // Try to find the record start
            if let Some(record_start) = self.find_record_start(data, absolute_pos) {
                trace!("Found record start at position {}", record_start);

                // Consume the complete record
                match self.consume_record(data, record_start) {
                    Ok(record_len) => {
                        // Apply size filters
                        if record_len >= self.config.min_descriptor_size
                            && record_len <= self.config.max_descriptor_size
                        {
                            let record_data = data[record_start..record_start + record_len].to_vec();
                            let range = record_start..record_start + record_len;

                            debug!(
                                "Found descriptor at {}..{} ({} bytes)",
                                range.start, range.end, record_len
                            );

                            results.push(ScanResult::new(record_data, range));

                            // Check if we've hit the limit
                            if self.config.max_results > 0
                                && results.len() >= self.config.max_results
                            {
                                break;
                            }

                            // Skip past this record
                            position = record_start + record_len;
                            continue;
                        }
                    }
                    Err(e) => {
                        trace!("Failed to consume record: {}", e);
                    }
                }
            }

            // Move past this .proto occurrence and continue searching
            position = absolute_pos + PROTO_SUFFIX.len();
        }

        debug!("Scan complete: found {} descriptors", results.len());
        Ok(results)
    }
}

/// Find a subsequence within a byte slice
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Scan a file for embedded protobuf descriptors
///
/// This is a convenience function that reads the file and scans it.
pub fn scan_file(path: impl AsRef<std::path::Path>) -> Result<Vec<ScanResult>> {
    let path = path.as_ref();
    let data = std::fs::read(path).map_err(|e| Error::file_read(path, e))?;
    Scanner::new().scan(&data)
}

/// Scan a file with custom configuration
pub fn scan_file_with_config(
    path: impl AsRef<std::path::Path>,
    config: ScannerConfig,
) -> Result<Vec<ScanResult>> {
    let path = path.as_ref();
    let data = std::fs::read(path).map_err(|e| Error::file_read(path, e))?;
    Scanner::with_config(config).scan(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_subsequence() {
        let data = b"hello.proto.world";
        assert_eq!(find_subsequence(data, b".proto"), Some(5));
        assert_eq!(find_subsequence(data, b"world"), Some(12));
        assert_eq!(find_subsequence(data, b"missing"), None);
    }

    #[test]
    fn test_scanner_config_builder() {
        let config = ScannerConfig::new()
            .max_results(10)
            .min_descriptor_size(20)
            .max_descriptor_size(1000);

        assert_eq!(config.max_results, 10);
        assert_eq!(config.min_descriptor_size, 20);
        assert_eq!(config.max_descriptor_size, 1000);
    }

    #[test]
    fn test_empty_input() {
        let scanner = Scanner::new();
        let results = scanner.scan(&[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_no_proto_suffix() {
        let scanner = Scanner::new();
        let data = b"this is just some random data without any protobuf content";
        let results = scanner.scan(data).unwrap();
        assert!(results.is_empty());
    }
}
