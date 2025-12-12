# Incremental Chunking System for Block Collection

## Overview

This document describes the incremental chunking system implemented for efficient block collection and storage. The system collects Bitcoin blocks from source files (including Start9 encrypted files) and automatically chunks, compresses, and moves them to secondary storage to prevent primary drive space exhaustion.

## Key Features

### 1. Incremental Chunking During Collection
- Blocks are collected and written to a temporary file
- When 125,000 blocks are collected, the chunk is automatically:
  - Compressed with `zstd -3` (optimized compression level)
  - Moved to secondary storage (`/run/media/acolyte/Extra/blockchain/`)
  - Temp file is truncated and cleared for the next chunk
- This prevents the primary drive from filling up during collection

### 2. Corruption Prevention
- **Pre-write validation**: Blocks with invalid versions (>0x7fffffff) are detected and skipped before writing
- **Periodic integrity checks**: Every 10,000 blocks, the system verifies recently written blocks
- **Chunking validation**: During chunk creation, all blocks are validated for size and version
- **Corrupted block skipping**: Invalid blocks are skipped during chunking with detailed logging

### 3. Collection-Only Mode
- Fast block collection without full validation during collection
- Validation is deferred to chunking stage for better performance
- Allows collection to proceed even with edge cases in encrypted blocks

### 4. Performance Optimizations
- **I/O Buffering**: 128MB buffers for file I/O operations
- **Parallel file reading**: Up to 16 threads for parallel block file processing
- **Local cache**: Files are pre-copied to local cache for faster access (SSHFS optimization)
- **Streaming compression**: Blocks are compressed on-the-fly during chunking
- **Reduced allocations**: Pre-allocated vectors and fixed-size arrays where possible

## Architecture

### File Structure
```
~/.cache/blvm-bench/
├── blvm-bench-blocks-temp.bin          # Current chunk being collected
├── blvm-bench-blocks-temp.bin.meta     # Block count metadata (binary u64)
└── block-files-temp/                   # Local cache of source files

/run/media/acolyte/Extra/blockchain/
├── chunk_0.bin.zst                     # Compressed chunk 0 (125k blocks)
├── chunk_1.bin.zst                     # Compressed chunk 1 (125k blocks)
└── ...
```

### Block Format in Temp File
Each block is stored as:
- `[len: u32 (little-endian)][block_data: variable]`

### Chunk Format
Chunks are compressed with `zstd -3` and contain:
- Same format as temp file (length + data for each block)
- Blocks are validated during chunking
- Corrupted blocks are skipped and logged

## Usage

### Collection-Only Mode
```rust
use blvm_bench::collect_only::collect_blocks_only;

// Collect blocks without validation (fast mode)
collect_blocks_only(
    Some(PathBuf::from("/path/to/bitcoin/blocks")),
    None
)?;
```

### Resuming Collection
The system automatically resumes from the last collected block count:
- Reads `blvm-bench-blocks-temp.bin.meta` for block count
- Estimates starting file index (conservative to avoid skipping files)
- Continues collection from where it left off

## Configuration

### Constants (in `block_file_reader.rs`)
- `INCREMENTAL_CHUNK_SIZE`: 125,000 blocks per chunk
- `TEMP_FILE_INTEGRITY_CHECK_INTERVAL`: 10,000 blocks
- `SECONDARY_CHUNK_DIR`: `/run/media/acolyte/Extra/blockchain`
- `MAX_VALID_BLOCK_SIZE`: 10MB
- `MIN_VALID_BLOCK_SIZE`: 88 bytes

## Error Handling

- **Corrupted blocks**: Skipped with warning, collection continues
- **File I/O errors**: Process fails immediately (data integrity critical)
- **Chunking errors**: Process fails, but existing chunks are preserved
- **Integrity check failures**: Logged as warnings in collection-only mode, full validation during chunking

## Performance Characteristics

- **Collection speed**: ~100-300 blocks/sec (depends on source)
- **Chunking speed**: ~25,000 blocks/sec (compression + validation)
- **Disk usage**: Primary drive only holds one chunk (~80GB uncompressed)
- **Compression ratio**: ~5-6GB per 125k blocks (zstd -3)

## Future Improvements

- Track exact file index for more accurate resume
- Parallel chunking for multiple chunks
- Incremental validation during collection (optional)
- Support for different chunk sizes based on available space
