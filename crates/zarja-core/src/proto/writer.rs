//! Extensible proto writing traits.
//!
//! This module provides the [`ProtoWriter`] trait for customizing
//! how proto elements are written to output.

use prost_types::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    MethodDescriptorProto, OneofDescriptorProto, ServiceDescriptorProto,
};
use std::fmt::Result;

/// Trait for writing proto elements to output.
///
/// Implement this trait to customize the output format for proto definitions.
/// The default implementation writes standard `.proto` syntax.
///
/// # Example
///
/// ```ignore
/// use protodump_core::proto::ProtoWriter;
///
/// struct JsonProtoWriter {
///     output: String,
/// }
///
/// impl ProtoWriter for JsonProtoWriter {
///     fn write_message(&mut self, message: &DescriptorProto) -> Result {
///         // Write message as JSON
///         self.output.push_str(&format!(r#"{{"name": "{}"}}"#, message.name()));
///         Ok(())
///     }
///     // ... implement other methods
/// }
/// ```
pub trait ProtoWriter {
    /// Write the complete file descriptor
    fn write_file(&mut self, file: &FileDescriptorProto) -> Result {
        let _ = file;
        Ok(())
    }

    /// Write a message definition
    fn write_message(&mut self, message: &DescriptorProto) -> Result {
        let _ = message;
        Ok(())
    }

    /// Write a field definition
    fn write_field(&mut self, field: &FieldDescriptorProto) -> Result {
        let _ = field;
        Ok(())
    }

    /// Write an enum definition
    fn write_enum(&mut self, enum_type: &EnumDescriptorProto) -> Result {
        let _ = enum_type;
        Ok(())
    }

    /// Write a service definition
    fn write_service(&mut self, service: &ServiceDescriptorProto) -> Result {
        let _ = service;
        Ok(())
    }

    /// Write a method definition
    fn write_method(&mut self, method: &MethodDescriptorProto) -> Result {
        let _ = method;
        Ok(())
    }

    /// Write a oneof definition
    fn write_oneof(&mut self, oneof: &OneofDescriptorProto) -> Result {
        let _ = oneof;
        Ok(())
    }
}

/// A no-op writer that discards all output
pub struct NullWriter;

impl ProtoWriter for NullWriter {}

/// A writer that collects statistics about the proto file
#[derive(Debug, Default)]
pub struct StatsWriter {
    /// Number of messages
    pub message_count: usize,
    /// Number of fields
    pub field_count: usize,
    /// Number of enums
    pub enum_count: usize,
    /// Number of services
    pub service_count: usize,
    /// Number of methods
    pub method_count: usize,
}

impl ProtoWriter for StatsWriter {
    fn write_message(&mut self, _message: &DescriptorProto) -> Result {
        self.message_count += 1;
        Ok(())
    }

    fn write_field(&mut self, _field: &FieldDescriptorProto) -> Result {
        self.field_count += 1;
        Ok(())
    }

    fn write_enum(&mut self, _enum_type: &EnumDescriptorProto) -> Result {
        self.enum_count += 1;
        Ok(())
    }

    fn write_service(&mut self, _service: &ServiceDescriptorProto) -> Result {
        self.service_count += 1;
        Ok(())
    }

    fn write_method(&mut self, _method: &MethodDescriptorProto) -> Result {
        self.method_count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_writer() {
        let mut writer = NullWriter;
        assert!(writer.write_file(&FileDescriptorProto::default()).is_ok());
    }

    #[test]
    fn test_stats_writer() {
        let mut writer = StatsWriter::default();
        writer.write_message(&DescriptorProto::default()).unwrap();
        writer.write_message(&DescriptorProto::default()).unwrap();
        writer.write_field(&FieldDescriptorProto::default()).unwrap();
        
        assert_eq!(writer.message_count, 2);
        assert_eq!(writer.field_count, 1);
    }
}
