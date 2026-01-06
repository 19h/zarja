# zarja

Extract Protocol Buffer definitions from compiled binaries.

When you compile a program that uses protobufs (Go, C++, Java, etc.), the `.proto` definitions often get embedded in the binary as `FileDescriptorProto` messages. zarja finds these embedded descriptors and reconstructs the original `.proto` source files.

## Why

You have a binary. You want to know what protobufs it uses. Maybe you're reverse engineering an API, analyzing a closed-source application, or recovering lost proto files from old builds. zarja extracts what's there.

## Installation

```bash
cargo install zarja

# or build from source
cargo build --release
./target/release/zarja --help
```

## Usage

### Single binary

```bash
# Extract all protos to current directory
zarja -f ./some-binary

# Extract to specific directory
zarja -f ./some-binary -o ./protos

# Just list what's in there
zarja -f ./some-binary --list-only
```

### Directory of binaries

```bash
# Recursively process all binaries in a directory
zarja -d /path/to/binaries -o ./protos

# See what's happening
zarja -d /path/to/binaries -o ./protos -v
```

### Output

```
$ zarja -f ./GeoServices -o ./protos --force -v
Wrote ./protos/AddressObject.proto
Wrote ./protos/geo3-slim.proto
Wrote ./protos/google/protobuf/descriptor.proto
Wrote ./protos/compressed_path.proto
Wrote ./protos/path.proto
INFO Summary: 6 found, 0 duplicates skipped, 1 conflicts renamed, 6 written
```

## How it works

### Finding descriptors

Protobuf's `FileDescriptorProto` always starts with field 1 (the filename), which is a length-delimited string ending in `.proto`. zarja scans the binary for the `.proto` byte sequence, backtracks to find the field header (`0x0A`), then parses forward using protobuf's wire format to find where the descriptor ends.

```
Binary data:
... garbage ... 0A 12 74 65 73 74 2E 70 72 6F 74 6F ... more fields ... garbage ...
                │  │  └──────── "test.proto" ────────┘
                │  └─ length: 18 bytes
                └─ field 1, wire type 2 (length-delimited)
```

The tricky part: binaries contain lots of noise, and descriptors can be adjacent to each other or surrounded by garbage. zarja's scanner handles edge cases like:

- Filenames exactly 10 bytes long (where the length byte is also `0x0A`)
- Adjacent descriptors that share boundaries
- Corrupted or partial descriptors (skipped gracefully)

### Reconstructing proto source

Once zarja has the raw `FileDescriptorProto` bytes, it parses them with prost and walks the descriptor tree to emit valid `.proto` syntax:

```
FileDescriptorProto
├── name: "example.proto"
├── package: "myapp"
├── message_type[]
│   └── DescriptorProto
│       ├── name: "Request"
│       ├── field[]
│       │   └── FieldDescriptorProto { name: "id", number: 1, type: INT32 }
│       └── nested_type[]
└── enum_type[]
```

Becomes:

```protobuf
syntax = "proto3";

package myapp;

message Request {
  int32 id = 1;
}
```

The reconstructor handles proto2 vs proto3 syntax, nested messages, enums, oneofs, maps, services, extensions, reserved fields, and most field options.

## Conflict resolution

When processing multiple binaries, you'll often find the same `.proto` file in several of them. Sometimes they're identical (duplicates), sometimes they differ (conflicts). zarja tracks content by hash and handles both:

| Situation | Behavior |
|-----------|----------|
| Same filename, same content | Skip (duplicate) |
| Same filename, different content | Rename with suffix |

Three strategies for handling conflicts:

```bash
# Append content hash (default): descriptor~a1b2c3d4.proto
zarja -d ./bins -o ./protos --conflict-strategy hash-suffix

# Append source binary name: descriptor~from-myapp.proto  
zarja -d ./bins -o ./protos --conflict-strategy source-suffix

# Keep first, skip rest
zarja -d ./bins -o ./protos --conflict-strategy skip-conflicts
```

## Binary detection

When scanning directories, zarja needs to figure out which files are actually binaries worth scanning. It uses a combination of:

1. **Extension filtering** - skips `.txt`, `.json`, `.py`, `.proto`, etc.
2. **Size filtering** - skips files < 1KB or > 500MB
3. **Magic bytes** - looks for Mach-O (`0xCFFAEDFE`), ELF (`0x7F454C46`), PE (`MZ`)
4. **Fallback** - tries files with no extension

## Project structure

```
zarja/
├── crates/
│   ├── zarja-core/          # Library: scanner + reconstructor
│   │   ├── scanner/         # Binary scanning, wire format parsing
│   │   ├── proto/           # Proto reconstruction, source generation
│   │   └── error.rs         # Error types
│   └── zarja-cli/           # Binary: CLI interface
```

### Using as a library

```rust
use zarja_core::{Scanner, ScanStrategy, ProtoReconstructor};

let data = std::fs::read("./binary")?;
let scanner = Scanner::new();

for result in scanner.scan(&data)? {
    match ProtoReconstructor::from_bytes(result.as_bytes()) {
        Ok(proto) => {
            println!("// {}", proto.filename());
            println!("{}", proto.reconstruct());
        }
        Err(e) => eprintln!("Failed to parse: {}", e),
    }
}
```

## Limitations

**What gets embedded depends on the language and build:**

- **Go**: Usually embeds full descriptors for reflection. Good extraction results.
- **C++**: Depends on build flags. Sometimes only has partial descriptors or none.
- **Java**: Often embeds descriptors. Results vary by protobuf version.

**What zarja can't recover:**

- Comments from the original `.proto` files (not stored in descriptors)
- Original formatting and whitespace
- Import paths may be incomplete if dependencies weren't embedded
- Custom options beyond the standard set

**Known gaps in reconstruction:**

- Some complex custom options aren't fully rendered
- `optimize_for`, `deprecated`, and a few other file options are TODOs
- Group fields (deprecated proto2 feature) are parsed but output is minimal

## Options

```
-f, --file <FILE>           Single binary to process
-d, --directory <DIR>       Directory of binaries (recursive)
-o, --output <DIR>          Output directory [default: .]
-v, --verbose               Increase verbosity (-v, -vv, -vvv)
    --force                 Overwrite existing files
    --dry-run               Show what would be extracted
    --list-only             List proto filenames only
    --max-descriptors <N>   Limit descriptors per file (0 = unlimited)
    --conflict-strategy     hash-suffix | source-suffix | skip-conflicts
    --format                proto | filename
```

## Examples

**Recover protos from a macOS system framework:**

```bash
zarja -f /System/Library/PrivateFrameworks/GeoServices.framework/GeoServices \
      -o ./apple-protos --force
```

**Scan an Android APK's native libraries:**

```bash
unzip app.apk -d ./unpacked
zarja -d ./unpacked/lib -o ./protos -v
```

**Quick inventory of what's in a binary:**

```bash
zarja -f ./mystery-binary --list-only
```

**Diff proto versions between two builds:**

```bash
zarja -f ./v1/server -o ./v1-protos
zarja -f ./v2/server -o ./v2-protos
diff -r ./v1-protos ./v2-protos
```

## Performance

zarja processes a ~35MB binary in about 40ms on an M1 Mac. The scanner is single-pass and reconstruction is straightforward tree traversal. Memory usage is proportional to binary size (it reads the whole file into memory).

## Building

```bash
git clone https://github.com/example/zarja
cd zarja
cargo build --release
cargo test
```

Minimum Rust version: 1.75

## License

MIT
