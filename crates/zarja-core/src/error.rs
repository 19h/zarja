//! Error types for the zarja-core library.
//!
//! This module provides comprehensive error handling using the `thiserror` crate,
//! with detailed error variants for different failure modes.

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for zarja operations
pub type Result<T> = std::result::Result<T, Error>;

/// Comprehensive error type for all zarja operations
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// Failed to read input file
    #[error("failed to read file '{path}': {source}")]
    FileRead {
        /// Path to the file that failed to read
        path: PathBuf,
        /// Underlying I/O error
        #[source]
        source: std::io::Error,
    },

    /// Failed to write output file
    #[error("failed to write file '{path}': {source}")]
    FileWrite {
        /// Path to the file that failed to write
        path: PathBuf,
        /// Underlying I/O error
        #[source]
        source: std::io::Error,
    },

    /// Failed to create output directory
    #[error("failed to create directory '{path}': {source}")]
    DirectoryCreate {
        /// Path to the directory that failed to create
        path: PathBuf,
        /// Underlying I/O error
        #[source]
        source: std::io::Error,
    },

    /// Path traversal attempt detected (security error)
    #[error("path traversal detected: '{path}' would escape output directory")]
    PathTraversal {
        /// The suspicious path
        path: PathBuf,
    },

    /// Invalid protobuf wire format
    #[error("invalid protobuf wire format at offset {offset}: {details}")]
    InvalidWireFormat {
        /// Byte offset where the error occurred
        offset: usize,
        /// Detailed description of the issue
        details: String,
    },

    /// Failed to decode varint
    #[error("failed to decode varint at offset {offset}: buffer too small or invalid encoding")]
    VarintDecode {
        /// Byte offset where the error occurred
        offset: usize,
    },

    /// Failed to parse FileDescriptorProto
    #[error("failed to parse FileDescriptorProto: {0}")]
    DescriptorParse(#[from] prost::DecodeError),

    /// Failed to build file descriptor with prost-reflect
    #[error("failed to build file descriptor: {0}")]
    DescriptorBuild(String),

    /// No descriptors found in input
    #[error("no protobuf descriptors found in input")]
    NoDescriptorsFound,

    /// Invalid field number in descriptor
    #[error("invalid field number {number}: must be between 1 and {max}")]
    InvalidFieldNumber {
        /// The invalid field number
        number: u32,
        /// Maximum valid field number
        max: u32,
    },

    /// Unsupported proto syntax version
    #[error("unsupported proto syntax: '{syntax}'")]
    UnsupportedSyntax {
        /// The unsupported syntax string
        syntax: String,
    },

    /// Generic internal error
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Creates a new file read error
    pub fn file_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::FileRead {
            path: path.into(),
            source,
        }
    }

    /// Creates a new file write error
    pub fn file_write(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::FileWrite {
            path: path.into(),
            source,
        }
    }

    /// Creates a new directory creation error
    pub fn directory_create(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::DirectoryCreate {
            path: path.into(),
            source,
        }
    }

    /// Creates a new path traversal error
    pub fn path_traversal(path: impl Into<PathBuf>) -> Self {
        Self::PathTraversal { path: path.into() }
    }

    /// Creates a new wire format error
    pub fn invalid_wire_format(offset: usize, details: impl Into<String>) -> Self {
        Self::InvalidWireFormat {
            offset,
            details: details.into(),
        }
    }

    /// Creates a new varint decode error
    pub fn varint_decode(offset: usize) -> Self {
        Self::VarintDecode { offset }
    }

    /// Creates a new descriptor build error
    pub fn descriptor_build(msg: impl Into<String>) -> Self {
        Self::DescriptorBuild(msg.into())
    }

    /// Creates a new internal error
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// Returns true if this is a recoverable error that should be skipped
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::DescriptorParse(_) | Self::DescriptorBuild(_) | Self::InvalidWireFormat { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = Error::path_traversal("/etc/passwd");
        assert!(err.to_string().contains("path traversal"));
        assert!(err.to_string().contains("/etc/passwd"));
    }

    #[test]
    fn test_is_recoverable() {
        assert!(Error::descriptor_build("test").is_recoverable());
        assert!(!Error::path_traversal("/test").is_recoverable());
    }
}
