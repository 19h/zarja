//! # zarja-core
//!
//! A library for extracting and reconstructing Protocol Buffer definitions from compiled binaries.
//!
//! This crate provides the core functionality for:
//! - Scanning binary files for embedded protobuf file descriptors
//! - Parsing raw protobuf wire format data
//! - Reconstructing human-readable `.proto` source files
//!
//! ## Architecture
//!
//! The library is organized into several modules:
//!
//! - [`scanner`]: Binary scanning and wire format parsing
//! - [`proto`]: Proto definition reconstruction
//! - [`error`]: Error types and handling
//!
//! ## Example
//!
//! ```no_run
//! use zarja_core::{Scanner, ScanStrategy, ProtoReconstructor};
//! use std::fs;
//!
//! // Read a binary file
//! let data = fs::read("./target/release/my_app")?;
//!
//! // Scan for embedded descriptors
//! let scanner = Scanner::new();
//! let results = scanner.scan(&data)?;
//!
//! // Reconstruct proto definitions
//! for result in results {
//!     if let Ok(reconstructor) = ProtoReconstructor::from_bytes(result.as_bytes()) {
//!         println!("{}", reconstructor.reconstruct());
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Extensibility
//!
//! The library provides several traits for customization:
//!
//! - [`ProtoWriter`]: Customize how proto elements are written
//! - [`ScanStrategy`]: Customize the binary scanning algorithm
//!

#![deny(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms, unreachable_pub)]

pub mod error;
pub mod proto;
pub mod scanner;

// Re-export primary types for convenience
pub use error::{Error, Result};
pub use proto::{NullWriter, ProtoReconstructor, ProtoWriter, ReconstructorConfig, StatsWriter};
pub use scanner::{ScanResult, ScanStrategy, Scanner, ScannerConfig};

/// Crate version for programmatic access
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum valid protobuf field number (2^29 - 1)
/// Used for `reserved X to max` ranges
pub const MAX_FIELD_NUMBER: u32 = 536_870_911;
