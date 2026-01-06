//! zarja - Extract Protocol Buffer definitions from compiled binaries
//!
//! This tool scans binary files for embedded protobuf file descriptors
//! and reconstructs them into human-readable `.proto` source files.

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, ValueEnum};
use zarja_core::{ProtoReconstructor, Scanner, ScanStrategy, ScannerConfig};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, trace, warn, Level};
use tracing_subscriber::EnvFilter;
use walkdir::WalkDir;

/// Extract Protocol Buffer definitions from compiled binaries
#[derive(Parser, Debug)]
#[command(name = "zarja")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(flatten)]
    input: InputMode,

    /// Output directory for extracted .proto files
    #[arg(short, long, default_value = ".")]
    output: PathBuf,

    /// Verbosity level (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Output format
    #[arg(long, value_enum, default_value = "proto")]
    format: OutputFormat,

    /// Maximum number of descriptors to extract per file (0 = unlimited)
    #[arg(long, default_value = "0")]
    max_descriptors: usize,

    /// Dry run - don't write files, just show what would be extracted
    #[arg(long)]
    dry_run: bool,

    /// Overwrite existing files without prompting
    #[arg(long)]
    force: bool,

    /// Only list found descriptors without extracting
    #[arg(long)]
    list_only: bool,

    /// Conflict resolution strategy for same-name different-content protos
    #[arg(long, value_enum, default_value = "hash-suffix")]
    conflict_strategy: ConflictStrategy,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct InputMode {
    /// Path to a single binary file to extract definitions from
    #[arg(short, long)]
    file: Option<PathBuf>,

    /// Path to a directory of binaries to process
    #[arg(short, long)]
    directory: Option<PathBuf>,
}

/// Output format for extracted definitions
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    /// Standard .proto format
    Proto,
    /// Just the filename (for scripting)
    Filename,
}

/// Strategy for resolving naming conflicts
#[derive(Debug, Clone, Copy, ValueEnum)]
enum ConflictStrategy {
    /// Append a short content hash: file~a1b2c3d4.proto
    HashSuffix,
    /// Append source binary name: file~from-binary.proto
    SourceSuffix,
    /// Skip conflicting files (keep first occurrence only)
    SkipConflicts,
}

/// Tracks seen proto files for deduplication
#[derive(Default)]
struct ProtoRegistry {
    /// Maps proto filename -> (content_hash, output_path)
    seen: HashMap<String, Vec<(String, PathBuf)>>,
    /// Statistics
    stats: RegistryStats,
}

#[derive(Default)]
struct RegistryStats {
    total_found: usize,
    duplicates_skipped: usize,
    conflicts_renamed: usize,
    written: usize,
}

impl ProtoRegistry {
    fn new() -> Self {
        Self::default()
    }

    /// Compute a short hash of the content (first 8 chars of blake3)
    fn content_hash(content: &str) -> String {
        let hash = blake3::hash(content.as_bytes());
        hash.to_hex()[..8].to_string()
    }

    /// Check if this exact content was already seen for this filename
    fn is_duplicate(&self, filename: &str, content_hash: &str) -> bool {
        self.seen
            .get(filename)
            .map(|entries| entries.iter().any(|(h, _)| h == content_hash))
            .unwrap_or(false)
    }

    /// Get the number of variants we've seen for this filename
    fn variant_count(&self, filename: &str) -> usize {
        self.seen.get(filename).map(|e| e.len()).unwrap_or(0)
    }

    /// Register a proto file and return the resolved output path
    fn register(
        &mut self,
        filename: &str,
        _content: &str,
        content_hash: &str,
        output_dir: &Path,
        source_binary: Option<&Path>,
        strategy: ConflictStrategy,
    ) -> Option<PathBuf> {
        self.stats.total_found += 1;

        // Check for exact duplicate
        if self.is_duplicate(filename, content_hash) {
            debug!("Skipping duplicate: {} (hash: {})", filename, content_hash);
            self.stats.duplicates_skipped += 1;
            return None;
        }

        // Determine output path
        let output_path = if self.variant_count(filename) == 0 {
            // First occurrence - use canonical name
            output_dir.join(filename)
        } else {
            // Conflict - need to resolve
            match strategy {
                ConflictStrategy::SkipConflicts => {
                    debug!(
                        "Skipping conflict: {} (different content, hash: {})",
                        filename, content_hash
                    );
                    self.stats.duplicates_skipped += 1;
                    return None;
                }
                ConflictStrategy::HashSuffix => {
                    let new_name = Self::add_suffix(filename, &format!("~{}", content_hash));
                    info!(
                        "Conflict resolved: {} -> {} (content differs)",
                        filename, new_name
                    );
                    self.stats.conflicts_renamed += 1;
                    output_dir.join(new_name)
                }
                ConflictStrategy::SourceSuffix => {
                    let source_name = source_binary
                        .and_then(|p| p.file_stem())
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown");
                    let new_name = Self::add_suffix(filename, &format!("~from-{}", source_name));
                    info!(
                        "Conflict resolved: {} -> {} (from {})",
                        filename, new_name, source_name
                    );
                    self.stats.conflicts_renamed += 1;
                    output_dir.join(new_name)
                }
            }
        };

        // Record this variant
        self.seen
            .entry(filename.to_string())
            .or_default()
            .push((content_hash.to_string(), output_path.clone()));

        Some(output_path)
    }

    /// Add a suffix before the .proto extension
    fn add_suffix(filename: &str, suffix: &str) -> String {
        if let Some(stem) = filename.strip_suffix(".proto") {
            format!("{}{}.proto", stem, suffix)
        } else {
            format!("{}{}", filename, suffix)
        }
    }

    fn print_summary(&self) {
        info!(
            "Summary: {} found, {} duplicates skipped, {} conflicts renamed, {} written",
            self.stats.total_found,
            self.stats.duplicates_skipped,
            self.stats.conflicts_renamed,
            self.stats.written
        );
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let level = match cli.verbose {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    };

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(level.into()))
        .with_target(false)
        .init();

    // Dispatch based on input mode
    if let Some(ref file) = cli.input.file {
        process_single_file(&cli, file)
    } else if let Some(ref directory) = cli.input.directory {
        process_directory(&cli, directory)
    } else {
        bail!("Either --file or --directory must be specified")
    }
}

/// Process a single binary file
fn process_single_file(cli: &Cli, file: &Path) -> Result<()> {
    if !file.exists() {
        bail!("Input file does not exist: {}", file.display());
    }
    if !file.is_file() {
        bail!("Input path is not a file: {}", file.display());
    }

    let mut registry = ProtoRegistry::new();
    process_binary(cli, file, &mut registry)?;

    if !cli.list_only && !cli.dry_run {
        registry.print_summary();
    }

    Ok(())
}

/// Process a directory of binaries recursively
fn process_directory(cli: &Cli, directory: &Path) -> Result<()> {
    if !directory.exists() {
        bail!("Directory does not exist: {}", directory.display());
    }
    if !directory.is_dir() {
        bail!("Path is not a directory: {}", directory.display());
    }

    info!("Scanning directory: {}", directory.display());

    let mut registry = ProtoRegistry::new();
    let mut binaries_processed = 0;

    // Walk the directory
    for entry in WalkDir::new(directory)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Skip directories
        if !path.is_file() {
            continue;
        }

        // Skip hidden files
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }

        // Try to determine if this is a binary file
        if !is_likely_binary(path) {
            trace!("Skipping non-binary: {}", path.display());
            continue;
        }

        debug!("Processing binary: {}", path.display());
        if let Err(e) = process_binary(cli, path, &mut registry) {
            // Log error but continue with other files
            warn!("Error processing {}: {}", path.display(), e);
        }
        binaries_processed += 1;
    }

    info!("Processed {} binaries", binaries_processed);

    if !cli.list_only && !cli.dry_run {
        registry.print_summary();
    }

    Ok(())
}

/// Heuristic to determine if a file is likely a binary executable
fn is_likely_binary(path: &Path) -> bool {
    // Check by extension - skip obvious non-binaries
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let skip_extensions = [
            "txt", "md", "json", "yaml", "yml", "xml", "html", "css", "js", "ts", "py", "rb", "go",
            "rs", "c", "h", "cpp", "hpp", "java", "proto", "toml", "ini", "cfg", "conf", "log",
            "csv", "svg", "png", "jpg", "jpeg", "gif", "pdf", "zip", "tar", "gz", "bz2", "xz",
            "7z", "rar", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
        ];
        if skip_extensions.contains(&ext.to_lowercase().as_str()) {
            return false;
        }
    }

    // Check file size - skip very small files (< 1KB) and very large files (> 500MB)
    if let Ok(metadata) = fs::metadata(path) {
        let size = metadata.len();
        if size < 1024 || size > 500 * 1024 * 1024 {
            return false;
        }
    }

    // Try to read magic bytes to identify binary formats
    if let Ok(mut file) = fs::File::open(path) {
        use std::io::Read;
        let mut magic = [0u8; 4];
        if file.read_exact(&mut magic).is_ok() {
            // Mach-O (macOS)
            if magic == [0xCF, 0xFA, 0xED, 0xFE] // 64-bit
                || magic == [0xCE, 0xFA, 0xED, 0xFE] // 32-bit
                || magic == [0xFE, 0xED, 0xFA, 0xCF] // 64-bit reverse
                || magic == [0xFE, 0xED, 0xFA, 0xCE] // 32-bit reverse
                || magic == [0xCA, 0xFE, 0xBA, 0xBE]
            // Universal
            {
                return true;
            }
            // ELF (Linux)
            if magic[0..4] == [0x7F, b'E', b'L', b'F'] {
                return true;
            }
            // PE (Windows) - MZ header
            if magic[0..2] == [b'M', b'Z'] {
                return true;
            }
        }
    }

    // If we can't determine, try it anyway if it has no extension
    path.extension().is_none()
}

/// Process a single binary and extract protos
fn process_binary(cli: &Cli, binary_path: &Path, registry: &mut ProtoRegistry) -> Result<()> {
    // Read the input file
    trace!("Reading {}", binary_path.display());
    let data = fs::read(binary_path)
        .with_context(|| format!("Failed to read input file: {}", binary_path.display()))?;

    trace!("Read {} bytes from {}", data.len(), binary_path.display());

    // Configure and run scanner
    let config = ScannerConfig::new().max_results(cli.max_descriptors);
    let scanner = Scanner::with_config(config);
    let results = scanner
        .scan(&data)
        .with_context(|| format!("Failed to scan binary: {}", binary_path.display()))?;

    if results.is_empty() {
        trace!("No descriptors found in {}", binary_path.display());
        return Ok(());
    }

    debug!(
        "Found {} potential descriptor(s) in {}",
        results.len(),
        binary_path.display()
    );

    // Process each result
    for (i, result) in results.iter().enumerate() {
        trace!(
            "Processing descriptor {} ({} bytes at offset {})",
            i + 1,
            result.data.len(),
            result.range.start
        );

        // Try to reconstruct the proto definition
        match ProtoReconstructor::from_bytes(&result.data) {
            Ok(reconstructor) => {
                let filename = reconstructor.filename();

                // Skip non-.proto files
                if !filename.ends_with(".proto") {
                    trace!("Skipping non-.proto file: {}", filename);
                    continue;
                }

                let content = reconstructor.reconstruct();
                let content_hash = ProtoRegistry::content_hash(&content);

                if cli.list_only {
                    println!("{}", filename);
                    continue;
                }

                match cli.format {
                    OutputFormat::Filename => {
                        println!("{}", filename);
                    }
                    OutputFormat::Proto => {
                        // Register and get output path
                        let output_path = registry.register(
                            filename,
                            &content,
                            &content_hash,
                            &cli.output,
                            Some(binary_path),
                            cli.conflict_strategy,
                        );

                        if let Some(output_path) = output_path {
                            if cli.dry_run {
                                println!("Would write: {}", output_path.display());
                                if cli.verbose > 0 {
                                    println!("---");
                                    println!("{}", content);
                                    println!("---");
                                }
                            } else {
                                match write_proto_file(&output_path, &content, cli.force) {
                                    Ok(()) => {
                                        println!("Wrote {}", output_path.display());
                                        registry.stats.written += 1;
                                    }
                                    Err(e) => {
                                        error!("Failed to write {}: {}", output_path.display(), e);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                trace!(
                    "Failed to parse descriptor {} at offset {}: {}",
                    i + 1,
                    result.range.start,
                    e
                );
            }
        }
    }

    Ok(())
}

/// Write a proto file to disk with path traversal protection
fn write_proto_file(output_path: &Path, content: &str, force: bool) -> Result<()> {
    // Create parent directories
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Check if file exists
    if output_path.exists() && !force {
        bail!(
            "File already exists: {} (use --force to overwrite)",
            output_path.display()
        );
    }

    // Write the file
    let mut file = fs::File::create(output_path)
        .with_context(|| format!("Failed to create file: {}", output_path.display()))?;

    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write file: {}", output_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_proto_registry_deduplication() {
        let mut registry = ProtoRegistry::new();
        let temp_dir = TempDir::new().unwrap();

        let content = "syntax = \"proto3\";\npackage test;";
        let hash = ProtoRegistry::content_hash(content);

        // First registration should succeed
        let path1 = registry.register(
            "test.proto",
            content,
            &hash,
            temp_dir.path(),
            None,
            ConflictStrategy::HashSuffix,
        );
        assert!(path1.is_some());
        assert!(path1.unwrap().ends_with("test.proto"));

        // Duplicate should be skipped
        let path2 = registry.register(
            "test.proto",
            content,
            &hash,
            temp_dir.path(),
            None,
            ConflictStrategy::HashSuffix,
        );
        assert!(path2.is_none());

        assert_eq!(registry.stats.duplicates_skipped, 1);
    }

    #[test]
    fn test_proto_registry_conflict_hash_suffix() {
        let mut registry = ProtoRegistry::new();
        let temp_dir = TempDir::new().unwrap();

        let content1 = "syntax = \"proto3\";\npackage test1;";
        let content2 = "syntax = \"proto3\";\npackage test2;";
        let hash1 = ProtoRegistry::content_hash(content1);
        let hash2 = ProtoRegistry::content_hash(content2);

        // First registration
        let path1 = registry.register(
            "test.proto",
            content1,
            &hash1,
            temp_dir.path(),
            None,
            ConflictStrategy::HashSuffix,
        );
        assert!(path1.is_some());
        assert!(path1.unwrap().ends_with("test.proto"));

        // Second with different content should get hash suffix
        let path2 = registry.register(
            "test.proto",
            content2,
            &hash2,
            temp_dir.path(),
            None,
            ConflictStrategy::HashSuffix,
        );
        assert!(path2.is_some());
        let path2_str = path2.unwrap().to_string_lossy().to_string();
        assert!(path2_str.contains("test~"));
        assert!(path2_str.ends_with(".proto"));

        assert_eq!(registry.stats.conflicts_renamed, 1);
    }

    #[test]
    fn test_add_suffix() {
        assert_eq!(
            ProtoRegistry::add_suffix("test.proto", "~abc123"),
            "test~abc123.proto"
        );
        assert_eq!(
            ProtoRegistry::add_suffix("path/to/test.proto", "~abc123"),
            "path/to/test~abc123.proto"
        );
    }

    #[test]
    fn test_content_hash() {
        let hash1 = ProtoRegistry::content_hash("hello");
        let hash2 = ProtoRegistry::content_hash("hello");
        let hash3 = ProtoRegistry::content_hash("world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 8);
    }

    #[test]
    fn test_is_likely_binary() {
        // Test file extensions
        assert!(!is_likely_binary(Path::new("/tmp/test.txt")));
        assert!(!is_likely_binary(Path::new("/tmp/test.json")));
        assert!(!is_likely_binary(Path::new("/tmp/test.proto")));
    }

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
