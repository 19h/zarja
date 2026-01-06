//! Proto definition reconstruction module.
//!
//! This module provides functionality to reconstruct human-readable `.proto`
//! source files from parsed `FileDescriptorProto` data.
//!
//! ## Architecture
//!
//! The reconstruction process is handled by [`ProtoReconstructor`], which:
//!
//! 1. Parses raw bytes into a `FileDescriptorProto`
//! 2. Builds a resolved `FileDescriptor` using prost-reflect
//! 3. Writes out the `.proto` source using the [`ProtoWriter`] trait
//!
//! ## Extensibility
//!
//! The [`ProtoWriter`] trait allows customization of how proto elements are written.
//! This can be used for alternative output formats (JSON, documentation, etc.).

mod writer;

use crate::error::{Error, Result};
use crate::MAX_FIELD_NUMBER;
use prost::Message;
use prost_reflect::{DescriptorPool, FileDescriptor};
use prost_types::FileDescriptorProto;
use std::fmt::Write as FmtWrite;

pub use writer::{NullWriter, ProtoWriter, StatsWriter};

/// Configuration for proto reconstruction
#[derive(Debug, Clone)]
pub struct ReconstructorConfig {
    /// Indentation string (default: 2 spaces)
    pub indent_str: String,
    /// Include source code comments if available
    pub include_comments: bool,
    /// Sort fields by number
    pub sort_fields: bool,
}

impl Default for ReconstructorConfig {
    fn default() -> Self {
        Self {
            indent_str: "  ".to_string(),
            include_comments: true,
            sort_fields: false,
        }
    }
}

impl ReconstructorConfig {
    /// Creates a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the indentation string
    pub fn indent_str(mut self, s: impl Into<String>) -> Self {
        self.indent_str = s.into();
        self
    }

    /// Sets whether to include comments
    pub fn include_comments(mut self, include: bool) -> Self {
        self.include_comments = include;
        self
    }

    /// Sets whether to sort fields by number
    pub fn sort_fields(mut self, sort: bool) -> Self {
        self.sort_fields = sort;
        self
    }
}

/// Proto syntax version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoSyntax {
    /// Proto2 syntax
    Proto2,
    /// Proto3 syntax
    Proto3,
}

impl ProtoSyntax {
    /// Returns the syntax declaration string
    pub fn as_str(&self) -> &'static str {
        match self {
            ProtoSyntax::Proto2 => "proto2",
            ProtoSyntax::Proto3 => "proto3",
        }
    }
}

impl TryFrom<&str> for ProtoSyntax {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "" | "proto2" => Ok(ProtoSyntax::Proto2),
            "proto3" => Ok(ProtoSyntax::Proto3),
            _ => Err(Error::UnsupportedSyntax {
                syntax: value.to_string(),
            }),
        }
    }
}

/// Reconstructs proto definitions from FileDescriptorProto
#[derive(Debug)]
pub struct ProtoReconstructor {
    /// The raw FileDescriptorProto
    proto: FileDescriptorProto,
    /// The resolved file descriptor
    descriptor: Option<FileDescriptor>,
    /// Configuration
    config: ReconstructorConfig,
}

impl ProtoReconstructor {
    /// Creates a new reconstructor from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let proto = FileDescriptorProto::decode(data)?;
        Self::from_proto(proto)
    }

    /// Creates a new reconstructor from a FileDescriptorProto
    pub fn from_proto(proto: FileDescriptorProto) -> Result<Self> {
        // Try to build a resolved descriptor
        let descriptor = Self::build_descriptor(&proto).ok();

        Ok(Self {
            proto,
            descriptor,
            config: ReconstructorConfig::default(),
        })
    }

    /// Creates a new reconstructor with custom config
    pub fn with_config(mut self, config: ReconstructorConfig) -> Self {
        self.config = config;
        self
    }

    /// Try to build a resolved FileDescriptor
    fn build_descriptor(proto: &FileDescriptorProto) -> Result<FileDescriptor> {
        // Create a FileDescriptorSet with just our file
        let fds = prost_types::FileDescriptorSet {
            file: vec![proto.clone()],
        };

        let mut fds_bytes = Vec::new();
        fds.encode(&mut fds_bytes).map_err(|e| {
            Error::descriptor_build(format!("failed to encode descriptor set: {}", e))
        })?;

        let pool = DescriptorPool::decode(fds_bytes.as_slice()).map_err(|e| {
            Error::descriptor_build(format!("failed to decode descriptor pool: {}", e))
        })?;

        // Get the file descriptor from the pool
        pool.get_file_by_name(proto.name())
            .ok_or_else(|| Error::descriptor_build("file not found in pool"))
    }

    /// Returns the original filename from the descriptor
    pub fn filename(&self) -> &str {
        self.proto.name()
    }

    /// Returns the computed output filename
    ///
    /// This parses the go_package option to extract the import path if present.
    pub fn output_filename(&self) -> String {
        if let Some(opts) = &self.proto.options {
            if let Some(go_package) = &opts.go_package {
                // go_package can be "import/path;package_name" or just "import/path"
                if let Some(idx) = go_package.find(';') {
                    let import_path = &go_package[..idx];
                    let base = std::path::Path::new(self.proto.name())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(self.proto.name());
                    return format!("{}/{}", import_path, base);
                }
            }
        }
        self.proto.name().to_string()
    }

    /// Returns the proto syntax version
    pub fn syntax(&self) -> ProtoSyntax {
        ProtoSyntax::try_from(self.proto.syntax()).unwrap_or(ProtoSyntax::Proto2)
    }

    /// Returns the resolved file descriptor if available
    ///
    /// The descriptor may not be available if the proto has unresolvable dependencies.
    pub fn file_descriptor(&self) -> Option<&FileDescriptor> {
        self.descriptor.as_ref()
    }

    /// Returns the raw FileDescriptorProto
    pub fn proto(&self) -> &FileDescriptorProto {
        &self.proto
    }

    /// Reconstruct the proto definition as a string
    pub fn reconstruct(&self) -> String {
        let mut output = String::new();
        self.write_to(&mut output).expect("String write cannot fail");
        output
    }

    /// Write the reconstructed proto to a writer
    pub fn write_to(&self, w: &mut impl FmtWrite) -> std::fmt::Result {
        let mut writer = DefaultProtoWriter::new(w, &self.config);
        writer.write_file(&self.proto, self.syntax())
    }
}

/// Default implementation of ProtoWriter
struct DefaultProtoWriter<'a, W: FmtWrite> {
    writer: &'a mut W,
    config: &'a ReconstructorConfig,
    indent_level: usize,
}

impl<'a, W: FmtWrite> DefaultProtoWriter<'a, W> {
    fn new(writer: &'a mut W, config: &'a ReconstructorConfig) -> Self {
        Self {
            writer,
            config,
            indent_level: 0,
        }
    }

    fn indent(&mut self) {
        self.indent_level += 1;
    }

    fn dedent(&mut self) {
        self.indent_level = self.indent_level.saturating_sub(1);
    }

    fn write_indent(&mut self) -> std::fmt::Result {
        for _ in 0..self.indent_level {
            write!(self.writer, "{}", self.config.indent_str)?;
        }
        Ok(())
    }

    fn writeln(&mut self, s: &str) -> std::fmt::Result {
        self.write_indent()?;
        writeln!(self.writer, "{}", s)
    }

    fn write_file(
        &mut self,
        proto: &FileDescriptorProto,
        syntax: ProtoSyntax,
    ) -> std::fmt::Result {
        // Syntax declaration
        writeln!(self.writer, "syntax = \"{}\";", syntax.as_str())?;
        writeln!(self.writer)?;

        // Package
        if !proto.package().is_empty() {
            writeln!(self.writer, "package {};", proto.package())?;
            writeln!(self.writer)?;
        }

        // File options
        self.write_file_options(proto)?;

        // Imports
        self.write_imports(proto)?;

        // Services
        for service in &proto.service {
            self.write_service(service)?;
        }

        // Messages
        for message in &proto.message_type {
            self.write_message(message, syntax)?;
        }

        // Enums
        for enum_type in &proto.enum_type {
            self.write_enum(enum_type)?;
        }

        // Extensions (top-level)
        for extension in &proto.extension {
            self.write_extension(extension, syntax)?;
        }

        Ok(())
    }

    fn write_file_options(&mut self, proto: &FileDescriptorProto) -> std::fmt::Result {
        let Some(opts) = &proto.options else {
            return Ok(());
        };

        let mut wrote_option = false;

        // Write known options
        macro_rules! write_string_option {
            ($name:expr, $value:expr) => {
                if let Some(v) = $value {
                    if !v.is_empty() {
                        writeln!(self.writer, "option {} = \"{}\";", $name, escape_string(v))?;
                        wrote_option = true;
                    }
                }
            };
        }

        macro_rules! write_bool_option {
            ($name:expr, $value:expr) => {
                if let Some(v) = $value {
                    writeln!(self.writer, "option {} = {};", $name, v)?;
                    wrote_option = true;
                }
            };
        }

        write_string_option!("java_package", opts.java_package.as_ref());
        write_string_option!("java_outer_classname", opts.java_outer_classname.as_ref());
        write_bool_option!("java_multiple_files", opts.java_multiple_files);
        write_bool_option!("java_string_check_utf8", opts.java_string_check_utf8);
        write_string_option!("go_package", opts.go_package.as_ref());
        write_bool_option!("cc_enable_arenas", opts.cc_enable_arenas);
        write_string_option!("objc_class_prefix", opts.objc_class_prefix.as_ref());
        write_string_option!("csharp_namespace", opts.csharp_namespace.as_ref());
        write_string_option!("swift_prefix", opts.swift_prefix.as_ref());
        write_string_option!("php_class_prefix", opts.php_class_prefix.as_ref());
        write_string_option!("php_namespace", opts.php_namespace.as_ref());
        write_string_option!("php_metadata_namespace", opts.php_metadata_namespace.as_ref());
        write_string_option!("ruby_package", opts.ruby_package.as_ref());

        if wrote_option {
            writeln!(self.writer)?;
        }

        Ok(())
    }

    fn write_imports(&mut self, proto: &FileDescriptorProto) -> std::fmt::Result {
        if proto.dependency.is_empty() {
            return Ok(());
        }

        // Build set of public and weak imports
        let public_deps: std::collections::HashSet<_> =
            proto.public_dependency.iter().map(|&i| i as usize).collect();
        let weak_deps: std::collections::HashSet<_> =
            proto.weak_dependency.iter().map(|&i| i as usize).collect();

        for (i, dep) in proto.dependency.iter().enumerate() {
            let modifier = if public_deps.contains(&i) {
                "public "
            } else if weak_deps.contains(&i) {
                "weak "
            } else {
                ""
            };
            writeln!(self.writer, "import {}\"{}\";", modifier, dep)?;
        }

        writeln!(self.writer)?;
        Ok(())
    }

    fn write_service(&mut self, service: &prost_types::ServiceDescriptorProto) -> std::fmt::Result {
        writeln!(self.writer, "service {} {{", service.name())?;
        self.indent();

        for method in &service.method {
            self.write_method(method)?;
        }

        self.dedent();
        writeln!(self.writer, "}}")?;
        writeln!(self.writer)?;
        Ok(())
    }

    fn write_method(&mut self, method: &prost_types::MethodDescriptorProto) -> std::fmt::Result {
        let client_streaming = method.client_streaming.unwrap_or(false);
        let server_streaming = method.server_streaming.unwrap_or(false);

        let input = if client_streaming {
            format!("stream {}", method.input_type())
        } else {
            method.input_type().to_string()
        };

        let output = if server_streaming {
            format!("stream {}", method.output_type())
        } else {
            method.output_type().to_string()
        };

        self.write_indent()?;
        writeln!(
            self.writer,
            "rpc {}({}) returns ({});",
            method.name(),
            input,
            output
        )?;

        Ok(())
    }

    fn write_message(
        &mut self,
        message: &prost_types::DescriptorProto,
        syntax: ProtoSyntax,
    ) -> std::fmt::Result {
        writeln!(self.writer, "message {} {{", message.name())?;
        self.indent();

        // Reserved ranges and names
        self.write_reserved(message)?;

        // Nested messages
        for nested in &message.nested_type {
            // Skip map entry types (they're synthetic)
            if nested.options.as_ref().map_or(false, |o| o.map_entry.unwrap_or(false)) {
                continue;
            }
            self.write_message(nested, syntax)?;
        }

        // Nested enums
        for enum_type in &message.enum_type {
            self.write_enum(enum_type)?;
        }

        // Collect oneof field indices
        let mut oneof_fields: std::collections::HashMap<i32, Vec<&prost_types::FieldDescriptorProto>> =
            std::collections::HashMap::new();

        for field in &message.field {
            if let Some(oneof_index) = field.oneof_index {
                // Check if this is a proto3 optional (has synthetic oneof)
                if !Self::is_proto3_optional(field, message) {
                    oneof_fields
                        .entry(oneof_index)
                        .or_default()
                        .push(field);
                }
            }
        }

        // Write oneofs
        for (i, oneof) in message.oneof_decl.iter().enumerate() {
            if let Some(fields) = oneof_fields.get(&(i as i32)) {
                if !fields.is_empty() {
                    self.write_oneof(oneof, fields, syntax)?;
                }
            }
        }

        // Write regular fields (excluding those in oneofs)
        for field in &message.field {
            let in_real_oneof = field.oneof_index.is_some()
                && !Self::is_proto3_optional(field, message)
                && oneof_fields.contains_key(&field.oneof_index.unwrap());

            if !in_real_oneof {
                self.write_field(field, syntax, message)?;
            }
        }

        // Extensions
        for extension in &message.extension {
            self.write_extension(extension, syntax)?;
        }

        // Extension ranges
        for range in &message.extension_range {
            self.write_indent()?;
            let end = if range.end() == MAX_FIELD_NUMBER as i32 + 1 {
                "max".to_string()
            } else {
                (range.end() - 1).to_string()
            };
            writeln!(self.writer, "extensions {} to {};", range.start(), end)?;
        }

        self.dedent();
        self.writeln("}")?;
        writeln!(self.writer)?;

        Ok(())
    }

    fn is_proto3_optional(
        field: &prost_types::FieldDescriptorProto,
        message: &prost_types::DescriptorProto,
    ) -> bool {
        // In proto3, optional fields have a synthetic oneof
        if let Some(oneof_index) = field.oneof_index {
            if let Some(oneof) = message.oneof_decl.get(oneof_index as usize) {
                // Synthetic oneofs have names starting with "_"
                return oneof.name().starts_with('_');
            }
        }
        false
    }

    fn write_reserved(&mut self, message: &prost_types::DescriptorProto) -> std::fmt::Result {
        // Reserved names
        if !message.reserved_name.is_empty() {
            self.write_indent()?;
            write!(self.writer, "reserved ")?;
            for (i, name) in message.reserved_name.iter().enumerate() {
                if i > 0 {
                    write!(self.writer, ", ")?;
                }
                write!(self.writer, "\"{}\"", name)?;
            }
            writeln!(self.writer, ";")?;
        }

        // Reserved ranges
        if !message.reserved_range.is_empty() {
            self.write_indent()?;
            write!(self.writer, "reserved ")?;
            for (i, range) in message.reserved_range.iter().enumerate() {
                if i > 0 {
                    write!(self.writer, ", ")?;
                }
                if range.start() == range.end() - 1 {
                    write!(self.writer, "{}", range.start())?;
                } else {
                    let end = if range.end() == MAX_FIELD_NUMBER as i32 + 1 {
                        "max".to_string()
                    } else {
                        (range.end() - 1).to_string()
                    };
                    write!(self.writer, "{} to {}", range.start(), end)?;
                }
            }
            writeln!(self.writer, ";")?;
        }

        Ok(())
    }

    fn write_oneof(
        &mut self,
        oneof: &prost_types::OneofDescriptorProto,
        fields: &[&prost_types::FieldDescriptorProto],
        _syntax: ProtoSyntax,
    ) -> std::fmt::Result {
        self.write_indent()?;
        writeln!(self.writer, "oneof {} {{", oneof.name())?;
        self.indent();

        for field in fields {
            self.write_oneof_field(field)?;
        }

        self.dedent();
        self.writeln("}")?;

        Ok(())
    }

    fn write_oneof_field(&mut self, field: &prost_types::FieldDescriptorProto) -> std::fmt::Result {
        self.write_indent()?;
        writeln!(
            self.writer,
            "{} {} = {};",
            self.field_type_name(field),
            field.name(),
            field.number()
        )?;
        Ok(())
    }

    fn write_field(
        &mut self,
        field: &prost_types::FieldDescriptorProto,
        syntax: ProtoSyntax,
        message: &prost_types::DescriptorProto,
    ) -> std::fmt::Result {
        self.write_indent()?;

        // Determine field label
        let label = self.field_label(field, syntax, message);
        if !label.is_empty() {
            write!(self.writer, "{} ", label)?;
        }

        // Check for map type
        if self.is_map_field(field, message) {
            self.write_map_field(field, message)?;
        } else {
            // Regular field
            write!(
                self.writer,
                "{} {} = {}",
                self.field_type_name(field),
                field.name(),
                field.number()
            )?;

            // Field options (default value, etc.)
            self.write_field_options(field, syntax)?;

            writeln!(self.writer, ";")?;
        }

        Ok(())
    }

    fn is_map_field(
        &self,
        field: &prost_types::FieldDescriptorProto,
        message: &prost_types::DescriptorProto,
    ) -> bool {
        if field.label() != prost_types::field_descriptor_proto::Label::Repeated {
            return false;
        }
        if field.r#type() != prost_types::field_descriptor_proto::Type::Message {
            return false;
        }

        // Find the nested type
        let type_name = field.type_name();
        for nested in &message.nested_type {
            let expected_name = format!(".{}", nested.name());
            if type_name.ends_with(&expected_name) || type_name == nested.name() {
                return nested.options.as_ref().map_or(false, |o| o.map_entry.unwrap_or(false));
            }
        }

        false
    }

    fn write_map_field(
        &mut self,
        field: &prost_types::FieldDescriptorProto,
        message: &prost_types::DescriptorProto,
    ) -> std::fmt::Result {
        // Find the map entry type
        let type_name = field.type_name();
        for nested in &message.nested_type {
            let expected_name = format!(".{}", nested.name());
            if type_name.ends_with(&expected_name) || type_name == nested.name() {
                if nested.options.as_ref().map_or(false, |o| o.map_entry.unwrap_or(false)) {
                    // This is a map entry
                    let key_field = nested.field.iter().find(|f| f.number() == 1);
                    let value_field = nested.field.iter().find(|f| f.number() == 2);

                    if let (Some(key), Some(value)) = (key_field, value_field) {
                        writeln!(
                            self.writer,
                            "map<{}, {}> {} = {};",
                            self.field_type_name(key),
                            self.field_type_name(value),
                            field.name(),
                            field.number()
                        )?;
                        return Ok(());
                    }
                }
            }
        }

        // Fallback: just write as a regular field
        writeln!(
            self.writer,
            "{} {} = {};",
            self.field_type_name(field),
            field.name(),
            field.number()
        )?;

        Ok(())
    }

    fn field_label(
        &self,
        field: &prost_types::FieldDescriptorProto,
        syntax: ProtoSyntax,
        message: &prost_types::DescriptorProto,
    ) -> &'static str {
        use prost_types::field_descriptor_proto::Label;

        match field.label() {
            Label::Repeated => {
                // Check if this is a map (maps don't get "repeated" label)
                if self.is_map_field(field, message) {
                    ""
                } else {
                    "repeated"
                }
            }
            Label::Required => "required",
            Label::Optional => {
                match syntax {
                    ProtoSyntax::Proto2 => "optional",
                    ProtoSyntax::Proto3 => {
                        // In proto3, check if this is an explicit optional (has synthetic oneof)
                        if Self::is_proto3_optional(field, message) {
                            "optional"
                        } else {
                            ""
                        }
                    }
                }
            }
        }
    }

    fn field_type_name(&self, field: &prost_types::FieldDescriptorProto) -> String {
        use prost_types::field_descriptor_proto::Type;

        match field.r#type() {
            Type::Double => "double".to_string(),
            Type::Float => "float".to_string(),
            Type::Int64 => "int64".to_string(),
            Type::Uint64 => "uint64".to_string(),
            Type::Int32 => "int32".to_string(),
            Type::Fixed64 => "fixed64".to_string(),
            Type::Fixed32 => "fixed32".to_string(),
            Type::Bool => "bool".to_string(),
            Type::String => "string".to_string(),
            Type::Bytes => "bytes".to_string(),
            Type::Uint32 => "uint32".to_string(),
            Type::Sfixed32 => "sfixed32".to_string(),
            Type::Sfixed64 => "sfixed64".to_string(),
            Type::Sint32 => "sint32".to_string(),
            Type::Sint64 => "sint64".to_string(),
            Type::Group => "group".to_string(),
            Type::Message | Type::Enum => {
                // Return the full type name
                field.type_name().to_string()
            }
        }
    }

    fn write_field_options(
        &mut self,
        field: &prost_types::FieldDescriptorProto,
        syntax: ProtoSyntax,
    ) -> std::fmt::Result {
        let mut options = Vec::new();

        // Default value (proto2 only)
        if syntax == ProtoSyntax::Proto2 {
            if let Some(default) = &field.default_value {
                use prost_types::field_descriptor_proto::Type;
                let formatted = match field.r#type() {
                    Type::String => format!("\"{}\"", escape_string(default)),
                    Type::Bytes => format!("\"{}\"", escape_string(default)),
                    Type::Enum => default.clone(),
                    Type::Bool => default.clone(),
                    _ => default.clone(),
                };
                options.push(format!("default = {}", formatted));
            }
        }

        // JSON name if different from default
        if let Some(json_name) = &field.json_name {
            let default_json_name = to_lower_camel_case(field.name());
            if json_name != &default_json_name {
                options.push(format!("json_name = \"{}\"", json_name));
            }
        }

        // Packed option
        if let Some(opts) = &field.options {
            if let Some(packed) = opts.packed {
                options.push(format!("packed = {}", packed));
            }
            if let Some(deprecated) = opts.deprecated {
                if deprecated {
                    options.push("deprecated = true".to_string());
                }
            }
        }

        if !options.is_empty() {
            write!(self.writer, " [{}]", options.join(", "))?;
        }

        Ok(())
    }

    fn write_enum(&mut self, enum_type: &prost_types::EnumDescriptorProto) -> std::fmt::Result {
        self.write_indent()?;
        writeln!(self.writer, "enum {} {{", enum_type.name())?;
        self.indent();

        // Check for allow_alias option
        if let Some(opts) = &enum_type.options {
            if opts.allow_alias.unwrap_or(false) {
                self.writeln("option allow_alias = true;")?;
            }
        }

        // Reserved ranges
        if !enum_type.reserved_range.is_empty() {
            self.write_indent()?;
            write!(self.writer, "reserved ")?;
            for (i, range) in enum_type.reserved_range.iter().enumerate() {
                if i > 0 {
                    write!(self.writer, ", ")?;
                }
                if range.start() == range.end() {
                    write!(self.writer, "{}", range.start())?;
                } else {
                    let end = if range.end() == i32::MAX {
                        "max".to_string()
                    } else {
                        range.end().to_string()
                    };
                    write!(self.writer, "{} to {}", range.start(), end)?;
                }
            }
            writeln!(self.writer, ";")?;
        }

        // Reserved names
        if !enum_type.reserved_name.is_empty() {
            self.write_indent()?;
            write!(self.writer, "reserved ")?;
            for (i, name) in enum_type.reserved_name.iter().enumerate() {
                if i > 0 {
                    write!(self.writer, ", ")?;
                }
                write!(self.writer, "\"{}\"", name)?;
            }
            writeln!(self.writer, ";")?;
        }

        // Values
        for value in &enum_type.value {
            self.write_indent()?;
            write!(self.writer, "{} = {}", value.name(), value.number())?;

            // Value options
            if let Some(opts) = &value.options {
                if opts.deprecated.unwrap_or(false) {
                    write!(self.writer, " [deprecated = true]")?;
                }
            }

            writeln!(self.writer, ";")?;
        }

        self.dedent();
        self.writeln("}")?;
        writeln!(self.writer)?;

        Ok(())
    }

    fn write_extension(
        &mut self,
        extension: &prost_types::FieldDescriptorProto,
        syntax: ProtoSyntax,
    ) -> std::fmt::Result {
        self.write_indent()?;
        writeln!(self.writer, "extend {} {{", extension.extendee())?;
        self.indent();

        self.write_indent()?;

        // Label
        use prost_types::field_descriptor_proto::Label;
        match extension.label() {
            Label::Repeated => write!(self.writer, "repeated ")?,
            Label::Required => write!(self.writer, "required ")?,
            Label::Optional => {
                if syntax == ProtoSyntax::Proto2 {
                    write!(self.writer, "optional ")?;
                }
            }
        }

        writeln!(
            self.writer,
            "{} {} = {};",
            self.field_type_name(extension),
            extension.name(),
            extension.number()
        )?;

        self.dedent();
        self.writeln("}")?;
        writeln!(self.writer)?;

        Ok(())
    }
}

/// Escape a string for proto syntax
fn escape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            _ if c.is_ascii_control() => {
                result.push_str(&format!("\\x{:02x}", c as u8));
            }
            _ => result.push(c),
        }
    }
    result
}

/// Convert a snake_case name to lowerCamelCase
fn to_lower_camel_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = false;

    for c in s.chars() {
        if c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(c.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_string() {
        assert_eq!(escape_string("hello"), "hello");
        assert_eq!(escape_string("hello\\world"), "hello\\\\world");
        assert_eq!(escape_string("hello\"world"), "hello\\\"world");
        assert_eq!(escape_string("hello\nworld"), "hello\\nworld");
    }

    #[test]
    fn test_to_lower_camel_case() {
        assert_eq!(to_lower_camel_case("hello_world"), "helloWorld");
        assert_eq!(to_lower_camel_case("my_field_name"), "myFieldName");
        assert_eq!(to_lower_camel_case("simple"), "simple");
    }

    #[test]
    fn test_proto_syntax() {
        assert_eq!(ProtoSyntax::try_from("").unwrap(), ProtoSyntax::Proto2);
        assert_eq!(ProtoSyntax::try_from("proto2").unwrap(), ProtoSyntax::Proto2);
        assert_eq!(ProtoSyntax::try_from("proto3").unwrap(), ProtoSyntax::Proto3);
        assert!(ProtoSyntax::try_from("proto4").is_err());
    }
}
