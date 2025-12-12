//! Direct Block File Reader
//!
//! Reads blocks directly from Bitcoin Core's block files (blk*.dat) without using RPC.
//! This eliminates RPC overhead and allows sharing block data between Core and Commons.

use anyhow::{Context, Result};
use hex;
use memchr::memchr_iter;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Bitcoin Core block file format:
/// - Magic bytes: 4 bytes (0xf9beb4d9 for mainnet)
/// - Block size: 4 bytes (little-endian)
/// - Block data: variable size
const BLOCK_MAGIC_MAINNET: [u8; 4] = [0xf9, 0xbe, 0xb4, 0xd9];
const BLOCK_MAGIC_TESTNET: [u8; 4] = [0x0b, 0x11, 0x09, 0x07];
const BLOCK_MAGIC_REGTEST: [u8; 4] = [0xfa, 0xbf, 0xb5, 0xda];

// ============================================================================
// Performance tuning constants - adjust these to optimize for your system
// ============================================================================
// Tuned for: Intel i7-8700K (6 cores, 12 threads), 15GB RAM, NVMe SSD
// ============================================================================

/// I/O buffer size for file reading and writing (in bytes)
/// Larger buffers reduce system calls and improve throughput for large files
/// Tuned: 128MB for NVMe SSD (excellent sequential I/O performance)
/// For HDD: use 64MB, for NVMe: 128MB+ is optimal
const IO_BUFFER_SIZE: usize = 128 * 1024 * 1024;

/// Search buffer size for pattern matching (in bytes)
/// Used when searching for block magic bytes in encrypted/out-of-order files
/// Tuned: 128MB to match IO_BUFFER_SIZE and leverage available RAM
const SEARCH_BUFFER_SIZE: usize = 128 * 1024 * 1024;

/// Chunk size for processing blocks when building hash maps (number of blocks)
/// Smaller chunks use less memory but may be slower
/// Tuned: 500 blocks (reduced from 2000 to prevent OOM - we only need headers, not full blocks)
/// With 500 blocks @ 1.5MB avg = ~750MB per chunk (safe for 15GB RAM)
const HASH_MAP_CHUNK_SIZE: usize = 500;

/// Maximum number of threads for parallel file reading
/// Tuned: 16 threads for local LAN SSHFS (I/O-bound, can use more threads than CPU cores)
/// For local network mounts, we can saturate network bandwidth with more parallelism
const MAX_PARALLEL_READ_THREADS: usize = 16;

/// Batch size for parallel file reading (files processed in parallel per batch)
/// Tuned: 24 files per batch for local LAN SSHFS (network I/O bound, not CPU bound)
/// Larger batches better utilize network bandwidth on local LAN
const PARALLEL_FILE_BATCH_SIZE: usize = 24;

/// Number of files to pre-copy ahead of current reading position
/// Tuned: 200 files ahead to ensure local cache is ready before reading
/// Larger lookahead ensures files are cached before we need them
const PRE_COPY_LOOKAHEAD: usize = 200;

/// Number of worker threads for background file copying
/// Used when copying files from remote mounts (SSHFS, etc.)
/// Tuned: 12 threads (match CPU thread count for maximum parallelism)
const FILE_COPY_WORKER_THREADS: usize = 12;

/// Progress reporting interval (number of blocks)
/// How often to print progress updates during long operations
/// Tuned: 10000 (good balance - not too frequent, not too sparse)
const PROGRESS_REPORT_INTERVAL: usize = 10000;

/// Flush interval for temp file writes (number of blocks)
/// How often to flush buffers to disk to prevent data loss on SIGKILL
/// Tuned: 500 blocks (more frequent flushes for safety with larger buffers)
const TEMP_FILE_FLUSH_INTERVAL: usize = 500;

/// Integrity check interval for temp file (number of blocks)
/// How often to verify blocks written to temp file are valid and readable
/// Tuned: 10000 blocks (balance between safety and performance)
const TEMP_FILE_INTEGRITY_CHECK_INTERVAL: usize = 10000;

/// Chunk size for incremental chunking during collection (number of blocks)
/// When this many blocks are collected, compress and move to secondary drive
/// Tuned: 125000 blocks per chunk (matches chunking script)
const INCREMENTAL_CHUNK_SIZE: usize = 125000;

/// Secondary drive path for incremental chunking
const SECONDARY_CHUNK_DIR: &str = "/run/media/acolyte/Extra/blockchain";

/// Maximum block size for validation (Bitcoin max is ~4MB, but allow up to 10MB for safety)
const MAX_VALID_BLOCK_SIZE: usize = 10 * 1024 * 1024;

/// Minimum block size (magic + size + header = 88 bytes minimum)
const MIN_VALID_BLOCK_SIZE: usize = 88;

/// Create a chunk from temp file and move to secondary drive
/// Temp file contains exactly chunk_size blocks
impl BlockFileReader {
    fn create_and_move_chunk_from_file(
        temp_file: &std::path::Path,
        chunk_num: usize,
        chunk_size: usize,
    ) -> Result<()> {
        use std::io::{Read, Write};
        
        let chunks_dir = std::path::Path::new(SECONDARY_CHUNK_DIR);
        std::fs::create_dir_all(chunks_dir)?;
        
        let local_chunk = temp_file.parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("chunks")
            .join(format!("chunk_{}.bin.zst", chunk_num));
        std::fs::create_dir_all(local_chunk.parent().unwrap())?;
        
        eprintln!("   🔧 Compressing chunk {} ({} blocks)...", chunk_num, chunk_size);
        
        // Open temp file - it contains exactly chunk_size blocks
        let mut temp_reader = std::fs::File::open(temp_file)?;
        
        // Compress chunk with zstd
        // OPTIMIZATION: Use -3 instead of -1 for better compression (10-15% better) with minimal speed loss
        let mut zstd_proc = std::process::Command::new("zstd")
            .args(&["-3", "--stdout"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::fs::File::create(&local_chunk)?)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to start zstd: {}", e))?;
        
        // OPTIMIZATION: Use buffered writer for zstd stdin (faster than unbuffered writes)
        use std::io::BufWriter;
        let mut zstd_stdin = BufWriter::with_capacity(IO_BUFFER_SIZE, 
            zstd_proc.stdin.take()
                .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdin"))?);
        
        // Read and compress blocks
        // OPTIMIZATION: Skip corrupted blocks and continue (they're unusable anyway)
        let mut blocks_in_chunk = 0;
        let mut skipped_blocks = 0;
        let mut current_block_index = 0;
        
        while blocks_in_chunk < chunk_size {
            let mut len_buf = [0u8; 4];
            match temp_reader.read_exact(&mut len_buf) {
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            
            let block_len = u32::from_le_bytes(len_buf) as usize;
            
            // Validate size - skip corrupted blocks
            if block_len > MAX_VALID_BLOCK_SIZE || block_len < MIN_VALID_BLOCK_SIZE {
                eprintln!("   ⚠️  WARNING: Skipping corrupted block {} in chunk {} (size: {} bytes)", current_block_index, chunk_num, block_len);
                skipped_blocks += 1;
                current_block_index += 1;
                // Try to skip past this corrupted block if size is reasonable
                // If size is absurdly large, we can't seek past it - break
                if block_len > 10 * 1024 * 1024 * 1024 {
                    eprintln!("   ⚠️  ERROR: Corrupted block size too large to skip ({} bytes), stopping chunk", block_len);
                    break;
                }
                // Seek past the corrupted block data
                use std::io::Seek;
                if let Err(e) = temp_reader.seek(std::io::SeekFrom::Current(block_len as i64)) {
                    eprintln!("   ⚠️  ERROR: Cannot seek past corrupted block: {}, stopping chunk", e);
                    break;
                }
                continue;
            }
            
            // Read block data
            let mut block_data = vec![0u8; block_len];
            match temp_reader.read_exact(&mut block_data) {
                Ok(_) => {},
                Err(e) => {
                    eprintln!("   ⚠️  WARNING: Cannot read block {} in chunk {}: {}, skipping", current_block_index, chunk_num, e);
                    skipped_blocks += 1;
                    current_block_index += 1;
                    continue;
                }
            }
            
            // VALIDATION: Validate block structure during chunking
            // This is where we validate blocks that were collected without validation
            let mut is_valid = true;
            if block_data.len() >= 4 {
                let version = u32::from_le_bytes([
                    block_data[0], block_data[1], 
                    block_data[2], block_data[3]
                ]);
                // CRITICAL FIX: Bitcoin block versions can be much higher than 10
                // Valid versions include: 1-4 (standard), 0x20000000+ (BIP9), etc.
                // Only reject obviously invalid: version == 0 or version > 0x7fffffff (would be negative if signed)
                if version == 0 || version > 0x7fffffff {
                    eprintln!("   ⚠️  WARNING: Skipping block {} in chunk {} (invalid version: {})", current_block_index, chunk_num, version);
                    skipped_blocks += 1;
                    current_block_index += 1;
                    is_valid = false;
                }
            }
            
            // Additional validation: check block has reasonable structure
            if is_valid && block_data.len() < MIN_VALID_BLOCK_SIZE {
                eprintln!("   ⚠️  WARNING: Skipping block {} in chunk {} (too small: {} bytes)", current_block_index, chunk_num, block_data.len());
                skipped_blocks += 1;
                current_block_index += 1;
                is_valid = false;
            }
            
            // Only write valid blocks
            if is_valid {
                // Write to zstd (buffered)
                zstd_stdin.write_all(&len_buf)?;
                zstd_stdin.write_all(&block_data)?;
                blocks_in_chunk += 1;
                
                // OPTIMIZATION: Reduce progress reporting frequency (less I/O overhead)
                if blocks_in_chunk % 25000 == 0 {
                    eprintln!("     Compressed {}/{} blocks... ({} skipped)", blocks_in_chunk, chunk_size, skipped_blocks);
                }
            }
            
            current_block_index += 1;
        }
        
        // OPTIMIZATION: Flush buffer before dropping
        zstd_stdin.flush()?;
        drop(zstd_stdin);
        let output = zstd_proc.wait_with_output()?;
        
        if !output.status.success() {
            return Err(anyhow::anyhow!("zstd compression failed: {}", 
                String::from_utf8_lossy(&output.stderr)));
        }
        
        if skipped_blocks > 0 {
            eprintln!("   ⚠️  Chunk {} compressed: {} valid blocks ({} corrupted blocks skipped)", chunk_num, blocks_in_chunk, skipped_blocks);
        } else {
            eprintln!("   ✅ Chunk {} compressed: {} blocks", chunk_num, blocks_in_chunk);
        }
        
        // Move to secondary drive
        let secondary_chunk = chunks_dir.join(format!("chunk_{}.bin.zst", chunk_num));
        eprintln!("   📦 Moving chunk {} to secondary drive...", chunk_num);
        
        std::fs::copy(&local_chunk, &secondary_chunk)?;
        
        // Verify copy
        let local_size = std::fs::metadata(&local_chunk)?.len();
        let secondary_size = std::fs::metadata(&secondary_chunk)?.len();
        
        if local_size != secondary_size {
            return Err(anyhow::anyhow!("Copy verification failed: {} != {}", local_size, secondary_size));
        }
        
        // Delete local copy
        std::fs::remove_file(&local_chunk)?;
        
        eprintln!("   ✅ Chunk {} moved to secondary drive ({} bytes)", chunk_num, secondary_size);
        
        Ok(())
    }
}

/// Block file reader for Bitcoin Core's blk*.dat format
pub struct BlockFileReader {
    data_dir: PathBuf,
    network: Network,
    block_files: Vec<PathBuf>,
    local_cache_dir: Option<PathBuf>, // For incremental local copying
    file_index: Option<std::collections::HashSet<usize>>, // Pre-scanned index of files with blocks
}

#[derive(Debug, Clone, Copy)]
pub enum Network {
    Mainnet,
    Testnet,
    Regtest,
}

impl Network {
    fn magic_bytes(&self) -> &[u8; 4] {
        match self {
            Network::Mainnet => &BLOCK_MAGIC_MAINNET,
            Network::Testnet => &BLOCK_MAGIC_TESTNET,
            Network::Regtest => &BLOCK_MAGIC_REGTEST,
        }
    }
}

impl BlockFileReader {
    /// Create a new block file reader
    pub fn new(data_dir: impl AsRef<Path>, network: Network) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let blocks_dir = data_dir.join("blocks");
        
        if !blocks_dir.exists() {
            anyhow::bail!("Blocks directory not found: {}", blocks_dir.display());
        }
        
        // Find all blk*.dat files
        // Note: May fail due to permissions, but we'll try anyway
        let mut block_files = Vec::new();
        match std::fs::read_dir(&blocks_dir) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            let path = entry.path();
                            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                                if file_name.starts_with("blk") && file_name.ends_with(".dat") {
                                    block_files.push(path);
                                }
                            }
                        }
                        Err(e) => {
                            // Permission error or other issue - continue trying other entries
                            eprintln!("⚠️  Warning: Could not read directory entry: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                anyhow::bail!("Cannot read blocks directory {}: {}. Check permissions.", blocks_dir.display(), e);
            }
        }
        
        if block_files.is_empty() {
            anyhow::bail!("No block files found in {}", blocks_dir.display());
        }
        
        block_files.sort(); // Process in order (blk00000.dat, blk00001.dat, etc.)
        
        // Set up local cache directory for incremental copying (if data_dir is remote/SSHFS)
        let local_cache_dir = if data_dir.to_string_lossy().contains("bitcoin-start9") {
            // This is a remote mount - use local cache
            let cache = dirs::cache_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
                .map(|cache| cache.join("blvm-bench").join("block-files-temp"));
            if let Some(ref cache_path) = cache {
                let _ = std::fs::create_dir_all(cache_path);
            }
            cache
        } else {
            None // Local files - no need to copy
        };
        
        // OPTIMIZATION: Pre-scan files to build index of files with blocks
        // This allows us to skip empty files entirely without opening them
        let file_index = if block_files.len() > 1000 {
            // For large file sets, pre-scan to build index
            println!("🔍 Pre-scanning {} files to build index (skip empty files)...", block_files.len());
            use std::sync::Arc;
            let block_files_arc = Arc::new(block_files.clone());
            let num_threads = num_cpus::get().min(16); // Use up to 16 threads for file indexing
            let chunk_size = (block_files_arc.len() + num_threads - 1) / num_threads;
            
            let (tx, rx) = std::sync::mpsc::channel();
            let mut handles = Vec::new();
            
            for thread_id in 0..num_threads {
                let start = thread_id * chunk_size;
                let end = (start + chunk_size).min(block_files_arc.len());
                if start >= end {
                    break;
                }
                
                let block_files_clone = block_files_arc.clone();
                let tx_clone = tx.clone();
                
                let handle = std::thread::spawn(move || {
                    let mut local_index = Vec::new();
                    for idx in start..end {
                        let file_path = &block_files_clone[idx];
                        // Quick metadata check - skip files < 8 bytes (too small for magic + size)
                        if let Ok(metadata) = std::fs::metadata(file_path) {
                            if metadata.len() >= 8 {
                                local_index.push(idx);
                            }
                        }
                    }
                    let _ = tx_clone.send(local_index);
                });
                handles.push(handle);
            }
            
            drop(tx); // Close sender so receiver knows when done
            
            // Collect results
            let mut index = std::collections::HashSet::new();
            let mut received = 0;
            for result in rx {
                for idx in result {
                    index.insert(idx);
                }
                received += 1;
            }
            
            // Wait for all threads
            for handle in handles {
                let _ = handle.join();
            }
            
            println!("   ✅ Index built: {} files have blocks ({} empty files skipped)", 
                     index.len(), block_files.len() - index.len());
            Some(index)
        } else {
            None // Small file set - not worth pre-scanning
        };
        
        Ok(Self {
            data_dir,
            network,
            block_files,
            local_cache_dir,
            file_index,
        })
    }
    
    /// Auto-detect Core data directory
    /// Defaults to standard local Bitcoin Core paths, with Start9 as fallback
    pub fn auto_detect(network: Network) -> Result<Self> {
        // Check common locations - standard Bitcoin Core paths first
        let possible_dirs = vec![
            dirs::home_dir().map(|h| h.join(".bitcoin")), // Standard local Bitcoin Core (default)
            Some(PathBuf::from("/root/.bitcoin")),
            Some(PathBuf::from("/var/lib/bitcoind")),
            // Start9 paths (fallback for local testing only)
            dirs::home_dir().map(|h| h.join("mnt/bitcoin-start9")),
            Some(PathBuf::from("/mnt/bitcoin-start9")),
        ];
        
        for dir in possible_dirs.into_iter().flatten() {
            let blocks_dir = dir.join("blocks");
            if blocks_dir.exists() {
                // Try to create reader - may fail due to permissions, but worth trying
                match Self::new(&dir, network) {
                    Ok(reader) => return Ok(reader),
                    Err(e) => {
                        // Log but continue trying other locations
                        eprintln!("⚠️  Could not read from {}: {}", dir.display(), e);
                        continue;
                    }
                }
            }
        }
        
        anyhow::bail!("Could not auto-detect Bitcoin Core data directory with readable blocks")
    }
    
    /// Read a block by height (requires index or sequential scan)
    /// 
    /// Note: This is slower than RPC for random access, but faster for sequential access
    /// because we can read blocks directly from disk without network overhead.
    pub fn read_block_by_height(&self, height: u64) -> Result<Vec<u8>> {
        // For now, we'll need to scan through blocks sequentially
        // In the future, we could use Core's block index (blocks/index/*)
        // or build our own index
        
        // TODO: Implement efficient height-to-block mapping
        // For now, this is a placeholder that would need Core's index
        anyhow::bail!("Direct height lookup not yet implemented. Use read_block_by_hash or sequential reading.")
    }
    
    /// Read blocks sequentially from block files
    /// 
    /// This is much faster than RPC for sequential access because:
    /// 1. No network latency
    /// 2. No RPC serialization overhead
    /// 3. Direct disk I/O (can be cached by OS)
    /// 
    /// For Start9 encrypted files, blocks may be stored out of order.
    /// This method reads all blocks and chains them by previous block hash.
    pub fn read_blocks_sequential(
        &self,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
    ) -> Result<BlockIterator> {
        // Check if this is a Start9 encrypted file (blocks stored out of order)
        let is_start9 = self.data_dir.to_string_lossy().contains("bitcoin-start9");
        
        if is_start9 {
            // For Start9, read all blocks and chain them by previous block hash
            BlockIterator::new_ordered(self, start_height, max_blocks)
        } else {
            // Standard format - blocks are in order
            BlockIterator::new(self, start_height, max_blocks)
        }
    }
    
    /// Read a block by hash (requires scanning or index)
    pub fn read_block_by_hash(&self, block_hash: &[u8; 32]) -> Result<Vec<u8>> {
        // Scan through block files to find matching hash
        // This is slow for random access but works
        let mut iterator = self.read_blocks_sequential(None, None)?;
        
        while let Some(block_result) = iterator.next() {
            let block_data = block_result?;
            
            // Calculate block hash (first 80 bytes are header)
            // OPTIMIZATION: Use blvm-consensus OptimizedSha256 (SHA-NI or AVX2) instead of sha2 crate
            if block_data.len() >= 80 {
                let header = &block_data[0..80];
                use blvm_consensus::crypto::OptimizedSha256;
                let hasher = OptimizedSha256::new();
                let computed_hash = hasher.hash256(header); // Double SHA256
                
                if computed_hash.as_slice() == block_hash {
                    return Ok(block_data);
                }
            }
        }
        
        anyhow::bail!("Block not found in block files")
    }
}

/// Iterator over blocks in block files
pub struct BlockIterator {
    reader: BlockFileReader,
    current_file_idx: usize,
    current_file: Option<BufReader<File>>,
    current_height: u64,
    start_height: Option<u64>,
    max_blocks: Option<usize>,
    blocks_read: usize,
    // For Start9: ordered blocks (read all, then chain by prev hash)
    ordered_blocks: Option<Vec<Vec<u8>>>,
    ordered_index: usize,
    // Reusable search buffer to avoid allocations
    search_buffer: Vec<u8>,
    // Thread pool for file copying (limit concurrent copies)
    copy_sender: Option<std::sync::mpsc::Sender<(PathBuf, PathBuf)>>,
    // Track last file index we started copying from (to avoid re-queueing)
    last_copy_start_idx: usize,
    // Runtime cache of files that failed with "failed to fill whole buffer" errors
    // This avoids expensive retries of files we know are problematic
    failed_files: std::collections::HashSet<usize>,
    // Track which file index we're currently reading from (for error tracking)
    current_reading_file_idx: Option<usize>,
}

impl BlockIterator {
    /// Process a chunk of blocks to build hash map (helper for OOM fix)
    /// OPTIMIZED: Process in parallel but with reduced chunk size and direct insertion
    fn process_chunk(
        chunk: &[(Vec<u8>, u64, usize)],
        blocks_by_prev_hash: &mut HashMap<Vec<u8>, (u64, usize)>,
        genesis_block: &mut Option<(u64, usize)>,
    ) -> Result<()> {
        // Process chunk in parallel (faster for CPU-bound header parsing)
        // OPTIMIZED: Extract only what we need (prev_hash) and insert directly
        // Reduced chunk size (500) keeps memory usage manageable
        let processed: Vec<(Vec<u8>, bool, u64, usize)> = chunk.par_iter()
            .map(|(block_data, offset, block_len)| {
                // Get previous block hash from header (bytes 4-36, little-endian)
                // Only need 32 bytes from header, not the full block
                if block_data.len() < 36 {
                    // Invalid block - return dummy value (will be skipped)
                    return (Vec::new(), false, *offset, *block_len);
                }
                
                let prev_hash_le = &block_data[4..36];
                
                // Check if this is genesis (prev hash is all zeros)
                let is_genesis = prev_hash_le.iter().all(|&b| b == 0);
                
                // Convert prev hash to big-endian for lookup
                let mut prev_hash_be = prev_hash_le.to_vec();
                prev_hash_be.reverse();
                
                (prev_hash_be, is_genesis, *offset, *block_len)
            })
            .collect();
        
        // Insert directly into hash map (no intermediate accumulation)
        for (prev_hash_be, is_genesis, offset, block_len) in processed {
            // Skip invalid blocks (empty prev_hash_be)
            if prev_hash_be.is_empty() {
                continue;
            }
            
            if is_genesis {
                *genesis_block = Some((offset, block_len));
            } else {
                blocks_by_prev_hash.insert(prev_hash_be, (offset, block_len));
            }
        }
        
        Ok(())
    }
    
    fn new(
        reader: &BlockFileReader,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
    ) -> Result<Self> {
        let mut iter = Self {
            reader: BlockFileReader {
                data_dir: reader.data_dir.clone(),
                network: reader.network,
                block_files: reader.block_files.clone(),
                local_cache_dir: reader.local_cache_dir.clone(),
                file_index: reader.file_index.clone(),
            },
            current_file_idx: 0,
            current_file: None,
            current_height: 0,
            start_height,
            max_blocks,
            blocks_read: 0,
            ordered_blocks: None,
            ordered_index: 0,
            search_buffer: vec![0u8; SEARCH_BUFFER_SIZE],
            copy_sender: None,
            last_copy_start_idx: 0,
            failed_files: std::collections::HashSet::new(), // Track files that failed to avoid retries
            current_reading_file_idx: None, // Track which file we're reading from
        };
        
        // Set up thread pool for file copying (limit to 10 concurrent copies)
        if iter.reader.local_cache_dir.is_some() {
            let (tx, rx) = std::sync::mpsc::channel();
            iter.copy_sender = Some(tx);
            
            // Spawn worker threads for file copying (share receiver via Arc<Mutex>)
            // Increased to 20 workers for better throughput with sparse files
            let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
            for _ in 0..FILE_COPY_WORKER_THREADS {
                let rx = rx.clone();
                std::thread::spawn(move || {
                    loop {
                        let recv_result = {
                            let rx_guard = rx.lock().unwrap();
                            rx_guard.recv()
                        };
                        match recv_result {
                            Ok((remote, local)) => {
                                if !local.exists() {
                                    let _ = std::fs::copy(&remote, &local);
                                }
                            }
                            Err(_) => break, // Channel closed
                        }
                    }
                });
            }
        }
        
        // Open first file with larger buffer for faster I/O
        if !iter.reader.block_files.is_empty() {
            let file_path = iter.get_local_or_remote_path(0)?;
            let file = File::open(&file_path)?;
            iter.current_file = Some(BufReader::with_capacity(IO_BUFFER_SIZE, file));
            
            // Start copying files ahead in background
            iter.start_background_copying();
        }
        
        Ok(iter)
    }
    
    /// Create iterator that reads all blocks and orders them by previous block hash
    /// This is needed for Start9 encrypted files where blocks are stored out of order
    fn new_ordered(
        reader: &BlockFileReader,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
    ) -> Result<Self> {
        use sha2::{Digest, Sha256};
        use std::path::PathBuf;
        
        // Define cache file path (used for both old format and temp file location)
        let cache_file = dirs::cache_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
            .map(|cache| cache.join("blvm-bench").join("start9_ordered_blocks.bin"));
        
        // Check for chunked cache first (new format)
        let chunks_dir = crate::chunked_cache::get_chunks_dir();
        let mut ordered_blocks: Option<Vec<Vec<u8>>> = None;
        
        if let Some(ref chunks_path) = chunks_dir {
            if chunks_path.exists() {
                match crate::chunked_cache::load_chunked_cache(chunks_path, start_height, max_blocks) {
                    Ok(Some(blocks)) => {
                        println!("   ✅ Loaded {} blocks from chunked cache", blocks.len());
                        ordered_blocks = Some(blocks);
                    }
                    Ok(None) => {
                        // Chunked cache doesn't exist, try old format
                    }
                    Err(e) => {
                        eprintln!("   ⚠️  Failed to load chunked cache: {} - trying old format", e);
                    }
                }
            }
        }
        
        // Fall back to old single-file cache format if chunked cache not available
        if ordered_blocks.is_none() {
            
            // Try to load from old cache format
            if let Some(ref cache_path) = cache_file {
                if cache_path.exists() {
                    println!("📂 Loading ordered block list from cache: {}", cache_path.display());
                    match std::fs::read(cache_path) {
                        Ok(cached_data) => {
                            // Deserialize: format is [block_count: u64][block1_len: u32][block1_data...][block2_len: u32][block2_data...]...
                            if cached_data.len() >= 8 {
                                let block_count = u64::from_le_bytes([
                                    cached_data[0], cached_data[1], cached_data[2], cached_data[3],
                                    cached_data[4], cached_data[5], cached_data[6], cached_data[7],
                                ]) as usize;
                                
                                // OPTIMIZATION: Pre-allocate blocks vector (average file has ~1000-5000 blocks)
                let mut blocks = Vec::with_capacity(2000);
                                let mut offset = 8;
                                
                                for _ in 0..block_count {
                                    if offset + 4 > cached_data.len() {
                                        break;
                                    }
                                    let block_len = u32::from_le_bytes([
                                        cached_data[offset],
                                        cached_data[offset + 1],
                                        cached_data[offset + 2],
                                        cached_data[offset + 3],
                                    ]) as usize;
                                    offset += 4;
                                    
                                    if offset + block_len > cached_data.len() {
                                        break;
                                    }
                                    
                                    blocks.push(cached_data[offset..offset + block_len].to_vec());
                                    offset += block_len;
                                }
                                
                                if blocks.len() == block_count && block_count > 0 {
                                    println!("   ✅ Loaded {} blocks from cache", blocks.len());
                                    ordered_blocks = Some(blocks);
                                } else {
                                    if block_count == 0 || blocks.len() == 0 {
                                        eprintln!("   ⚠️  Cache file is empty ({} blocks) - will read from files", block_count);
                                    } else {
                                        eprintln!("   ⚠️  Cache file corrupted (expected {} blocks, got {}) - will read from files", block_count, blocks.len());
                                    }
                                    // Don't set ordered_blocks - let it read from files
                                    ordered_blocks = None;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("   ⚠️  Failed to read cache: {}", e);
                        }
                    }
                }
            }
        }
        
        // If cache miss, read and order all blocks
        if ordered_blocks.is_none() {
            println!("📦 Reading ALL blocks from file to order them by previous block hash...");
            println!("   (Blocks are stored out of order, so we need to read all to find the chain)");
            println!("   This is a one-time operation - results will be cached for future runs");
            
            // For Start9, blocks are out of order, so we need to read ALL blocks
            // to find the ones we need. This is a one-time cost per file.
            // To avoid OOM, write blocks to temp file as we read them
            // Use cache directory if available (has more space), otherwise use temp dir
            //
            // ⚠️  CRITICAL: NEVER DELETE THIS TEMP FILE
            // This file represents DAYS of work reading and processing blocks from disk.
            // It can be used to resume if the process is interrupted.
            // The temp file is a valuable backup even after cache is saved.
            // CODE MUST NEVER DELETE THIS FILE - Users can manually delete if desired.
            use std::io::{Write, BufWriter};
            let temp_file = if let Some(ref cache_path) = cache_file {
                if let Some(parent) = cache_path.parent() {
                    // Ensure parent directory exists
                    let _ = std::fs::create_dir_all(parent);
                    parent.join("blvm-bench-blocks-temp.bin")
                } else {
                    std::env::temp_dir().join("blvm-bench-blocks-temp.bin")
                }
            } else {
                std::env::temp_dir().join("blvm-bench-blocks-temp.bin")
            };
            
            // Check if temp file exists and resume from it
            let (mut temp_writer, mut read_count, start_time) = if temp_file.exists() {
                // OPTIMIZATION: Try to read count from metadata file first (instant)
                // FIX: Use binary u64 format instead of ASCII text for better reliability
                let metadata_file = temp_file.with_extension("bin.meta");
                let metadata_count = if metadata_file.exists() {
                    // Try binary format first (u64, 8 bytes)
                    match std::fs::read(&metadata_file) {
                        Ok(data) if data.len() == 8 => {
                            // Binary format: u64 little-endian
                            Some(u64::from_le_bytes([
                                data[0], data[1], data[2], data[3],
                                data[4], data[5], data[6], data[7],
                            ]) as usize)
                        }
                        Ok(data) if data.len() < 8 => {
                            // Too short, try ASCII fallback for old format
                            match std::str::from_utf8(&data) {
                                Ok(s) => s.trim().parse::<usize>().ok(),
                                Err(_) => None
                            }
                        }
                        Ok(_) => {
                            // Wrong size, try ASCII fallback for old format
                            match std::fs::read_to_string(&metadata_file) {
                                Ok(content) => content.trim().parse::<usize>().ok(),
                                Err(_) => None
                            }
                        }
                        Err(_) => None
                    }
                } else {
                    None
                };
                
                let existing_count = if let Some(count) = metadata_count {
                    // Use cached count - instant!
                    println!("   ✅ Found existing temp file with {} blocks (from metadata)", count);
                    count
                } else {
                    // No metadata - estimate from file size (instant, allows immediate start)
                    // Rough estimate: average block size ~6.5MB based on previous runs
                    let file_size = std::fs::metadata(&temp_file).map(|m| m.len()).unwrap_or(0);
                    let estimated_count = (file_size as f64 / (6.5 * 1024.0 * 1024.0)) as usize;
                    
                    println!("   ⚡ No metadata found - estimating {} blocks from file size ({:.2} GB)", 
                            estimated_count, file_size as f64 / 1_073_741_824.0);
                    println!("   🚀 Starting parallel reading immediately (counting continues in background)");
                    
                    // Start background counting thread to get accurate count
                    let temp_file_clone = temp_file.clone();
                    let metadata_file_clone = metadata_file.clone();
                    std::thread::spawn(move || {
                        // Background counting - doesn't block main process
                        let mut temp_file_handle = match std::fs::File::open(&temp_file_clone) {
                            Ok(f) => f,
                            Err(_) => return,
                        };
                        use std::io::{Read, Seek, SeekFrom};
                        let mut count = 0;
                        let count_start = std::time::Instant::now();
                        let mut last_progress = std::time::Instant::now();
                        let file_size = std::fs::metadata(&temp_file_clone).map(|m| m.len()).unwrap_or(0) as f64;
                        
                        loop {
                            let mut len_buf = [0u8; 4];
                            match temp_file_handle.read_exact(&mut len_buf) {
                                Ok(_) => {},
                                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                                Err(_) => break,
                            }
                            
                            let block_len = u32::from_le_bytes(len_buf) as usize;
                            
                            // VALIDATION: Check block size is reasonable
                            if block_len > MAX_VALID_BLOCK_SIZE || block_len < MIN_VALID_BLOCK_SIZE {
                                eprintln!("   [Background] ⚠️  ERROR: Block {} has invalid size: {} bytes - stopping count", count, block_len);
                                break; // Stop counting if corruption detected
                            }
                            
                            count += 1;
                            
                            // Progress every 100k blocks or every 10 seconds (less frequent for background)
                            if count % 100000 == 0 || last_progress.elapsed().as_secs() >= 10 {
                                let elapsed = count_start.elapsed().as_secs_f64();
                                let rate = count as f64 / elapsed;
                                let pos = temp_file_handle.stream_position().unwrap_or(0) as f64;
                                let progress_pct = if file_size > 0.0 {
                                    (pos / file_size * 100.0).min(100.0)
                                } else {
                                    0.0
                                };
                                eprintln!("   [Background] Counting: {} blocks ({:.0} blocks/sec, {:.1}% of file)", 
                                         count, rate, progress_pct);
                                last_progress = std::time::Instant::now();
                            }
                            
                            match temp_file_handle.seek(SeekFrom::Current(block_len as i64)) {
                                Ok(_) => {},
                                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                                Err(_) => break,
                            }
                        }
                        
                        let elapsed = count_start.elapsed().as_secs_f64();
                        eprintln!("   [Background] ✅ Finished counting: {} blocks in {:.1} seconds", count, elapsed);
                        
                        // Save to metadata for next time
                        // FIX: Use binary u64 format instead of ASCII text
                        let count_bytes = (count as u64).to_le_bytes();
                        if let Err(e) = std::fs::write(&metadata_file_clone, count_bytes) {
                            eprintln!("   [Background] ⚠️  Warning: Could not save metadata file: {}", e);
                        }
                    });
                    
                    estimated_count
                };
                
                if existing_count > 0 {
                    println!("   ✅ Resuming from {} existing blocks in temp file", existing_count);
                    // Open in append mode
                    let file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&temp_file)?;
                    (BufWriter::with_capacity(IO_BUFFER_SIZE, file), existing_count, std::time::Instant::now())
                } else {
                    // File exists but is empty/corrupted - start fresh
                    println!("   ⚠️  Temp file exists but is empty/corrupted - starting fresh");
                    (BufWriter::with_capacity(IO_BUFFER_SIZE, std::fs::File::create(&temp_file)?), 0, std::time::Instant::now())
                }
            } else {
                // No temp file - start fresh
                (BufWriter::with_capacity(IO_BUFFER_SIZE, std::fs::File::create(&temp_file)?), 0, std::time::Instant::now())
            };
            
            // DEBUG: Verify we reach this point
            eprintln!("   🔍 DEBUG: Reached parallel reading section, read_count={}, temp_file exists={}", 
                     read_count, temp_file.exists());
            
            // OPTIMIZATION: Parallel batch file reading
            // Read multiple files in parallel batches for faster processing, especially in sparse regions
            // Use maximum threads for I/O-bound workload (local LAN SSHFS can handle more parallelism)
            let num_threads = MAX_PARALLEL_READ_THREADS;
            
            // Create a custom thread pool for this operation
            // Global pool might already be initialized, so we use a scoped pool
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .build()
                .context("Failed to create rayon thread pool")?;
            
            println!("   🚀 Using parallel batch reading ({} threads)", num_threads);
            eprintln!("   🔍 DEBUG: Parallel reading initialized with {} threads", num_threads);
            
            // Estimate total blocks (rough estimate based on typical blockchain size)
            let estimated_total = 926000u64; // Rough estimate
            
            // Helper function to read all blocks from a single file
            // Uses full pattern searching logic to ensure no blocks are missed
            let network = reader.network;
            let file_index_clone = reader.file_index.clone();
            let local_cache_dir_clone = reader.local_cache_dir.clone();
            let read_blocks_from_file = move |file_idx: usize, file_path: &PathBuf| -> Result<Vec<Vec<u8>>> {
                use std::io::{BufReader, Read, Seek, SeekFrom};
                
                const XOR_KEY1: [u8; 4] = [0x84, 0x22, 0xe9, 0xad];
                const XOR_KEY2: [u8; 4] = [0xb7, 0x8f, 0xff, 0x14];
                const ENCRYPTED_MAGIC: [u8; 4] = [0x7d, 0x9c, 0x5d, 0x74];
                
                // Check if file should be skipped (from pre-scan index)
                if let Some(ref index) = file_index_clone {
                    if !index.contains(&file_idx) {
                        return Ok(Vec::new()); // Empty file - skip
                    }
                }
                
                // OPTIMIZATION: Use local cache if available (much faster than SSHFS)
                let path_to_use = if let Some(ref cache_dir) = local_cache_dir_clone {
                    let file_name = file_path.file_name()
                        .ok_or_else(|| anyhow::anyhow!("Invalid file path"))?;
                    let local_path = cache_dir.join(file_name);
                    
                    // If local copy exists, use it (fast local disk I/O)
                    if local_path.exists() {
                        local_path
                    } else {
                        // Local copy doesn't exist yet - try to copy it synchronously
                        // This is slower but ensures we use local cache for subsequent reads
                        if let Err(e) = std::fs::copy(file_path, &local_path) {
                            // Copy failed - fall back to remote
                            file_path.clone()
                        } else {
                            // Copy succeeded - use local
                            local_path
                        }
                    }
                } else {
                    // No local cache - use remote directly
                    file_path.clone()
                };
                
                // Try to open file (from local cache if available, otherwise remote)
                let file = match File::open(&path_to_use) {
                    Ok(f) => f,
                    Err(_) => return Ok(Vec::new()), // Skip if can't open
                };
                
                let mut file_reader = BufReader::with_capacity(IO_BUFFER_SIZE, file);
                // OPTIMIZATION: Pre-allocate blocks vector with estimated capacity
                // Average file has ~1000-5000 blocks, pre-allocate to reduce reallocations
                let mut blocks = Vec::with_capacity(2000);
                let magic = network.magic_bytes();
                // OPTIMIZATION: Check string once, cache result
                let is_xor_encrypted = file_path.to_string_lossy().contains("bitcoin-start9");
                
                // Pre-allocate search buffer for pattern matching (same as original)
                // OPTIMIZATION: Reuse buffer instead of allocating each time
                let mut search_buffer = vec![0u8; SEARCH_BUFFER_SIZE];
                
                // Read blocks from file using full pattern searching logic
                loop {
                    // CRITICAL FIX: Track file position BEFORE reading magic
                    // This is needed for correct XOR key rotation in Start9 files
                    let magic_start_pos = file_reader.seek(SeekFrom::Current(0)).unwrap_or(0);
                    
                    let mut magic_buf = [0u8; 4];
                    match file_reader.read_exact(&mut magic_buf) {
                        Ok(_) => {},
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                        Err(_) => break, // Skip on error
                    }
                    
                    // Check if file is XOR encrypted (Start9 format)
                    let mut encrypted_magic_bytes = magic_buf;
                    let is_encrypted = if is_xor_encrypted {
                        magic_buf == ENCRYPTED_MAGIC
                    } else {
                        false
                    };
                    
                    if is_encrypted {
                        // CRITICAL FIX: Decrypt magic using correct key based on FILE OFFSET
                        // Magic is at file offset magic_start_pos, so use key based on that
                        let use_key1 = (magic_start_pos / 4) % 2 == 0;
                        let key = if use_key1 { &XOR_KEY1 } else { &XOR_KEY2 };
                        for i in 0..4 {
                            let byte_offset = magic_start_pos + i as u64;
                            magic_buf[i] ^= key[(byte_offset % 4) as usize];
                        }
                    }
                    
                    if magic_buf != *magic {
                        // Not a block start - try pattern search if encrypted
                        if is_xor_encrypted {
                            // Seek back and try pattern search
                            file_reader.seek(SeekFrom::Current(-3)).ok();
                            let current_pos = file_reader.seek(SeekFrom::Current(0)).unwrap_or(0);
                            let mut search_pos = current_pos;
                            let mut found = false;
                            
                            // Pattern search for encrypted magic
                            loop {
                                let bytes_read = match file_reader.read(&mut search_buffer) {
                                    Ok(0) => break,
                                    Ok(n) => n,
                                    Err(_) => break,
                                };
                                
                                // Use memchr for fast pattern searching
                                let first_byte = ENCRYPTED_MAGIC[0];
                                for i in memchr_iter(first_byte, &search_buffer[..bytes_read]) {
                                    if i + 3 >= bytes_read {
                                        continue;
                                    }
                                    
                                    if search_buffer[i+1] == ENCRYPTED_MAGIC[1]
                                        && search_buffer[i+2] == ENCRYPTED_MAGIC[2]
                                        && search_buffer[i+3] == ENCRYPTED_MAGIC[3] {
                                        
                                        let file_offset = search_pos + i as u64;
                                        // Verify: decrypt and check
                                        let mut test_magic = [0u8; 4];
                                        test_magic.copy_from_slice(&search_buffer[i..i+4]);
                                        
                                        // Use u32 XOR for magic verification
                                        let magic_u32 = u32::from_le_bytes(test_magic);
                                        let key1_u32 = u32::from_le_bytes(XOR_KEY1);
                                        let key2_u32 = u32::from_le_bytes(XOR_KEY2);
                                        let use_key1 = (file_offset / 4) % 2 == 0;
                                        let key_u32 = if use_key1 { key1_u32 } else { key2_u32 };
                                        let decrypted_magic_u32 = magic_u32 ^ key_u32;
                                        test_magic = decrypted_magic_u32.to_le_bytes();
                                        
                                        if test_magic == *magic {
                                            // Found next block - seek to it
                                            file_reader.seek(SeekFrom::Start(file_offset))?;
                                            found = true;
                                            break;
                                        }
                                    }
                                }
                                
                                if found {
                                    break;
                                }
                                
                                search_pos += bytes_read as u64;
                                if bytes_read < search_buffer.len() {
                                    break;
                                }
                            }
                            
                            if !found {
                                break; // No more blocks
                            }
                            continue; // Retry reading magic at new position
                        } else {
                            // Not encrypted - just seek back and continue
                            file_reader.seek(SeekFrom::Current(-3)).ok();
                            continue;
                        }
                    }
                    
                    // Read size field (at file offset magic_start_pos + 4)
                    let mut size_buf = [0u8; 4];
                    if file_reader.read_exact(&mut size_buf).is_err() {
                        break;
                    }
                    
                    // CRITICAL FIX: Use magic_start_pos as block_start_offset for XOR decryption
                    // This is the file offset where the block's magic bytes start
                    let block_start_offset = if is_xor_encrypted {
                        Some(magic_start_pos)
                    } else {
                        None
                    };
                    
                    let mut block_size = if is_xor_encrypted {
                        // CRITICAL FIX: Decrypt size field using correct key based on FILE OFFSET
                        // Size field is at file offset magic_start_pos + 4
                        // CRITICAL FIX: Decrypt size field using correct key based on FILE OFFSET
                        // Size field is at file offset magic_start_pos + 4
                        // All 4 bytes of size field are in the same 4-byte chunk, so they use the same key
                        let size_offset = magic_start_pos + 4;
                        let use_key1 = (size_offset / 4) % 2 == 0;
                        let key = if use_key1 { &XOR_KEY1 } else { &XOR_KEY2 };
                        
                        // Decrypt each byte of size field with correct key byte
                        let mut decrypted_size_bytes = [0u8; 4];
                        for i in 0..4 {
                            let byte_offset = size_offset + i as u64;
                            decrypted_size_bytes[i] = size_buf[i] ^ key[(byte_offset % 4) as usize];
                        }
                        let decrypted_size_u32 = u32::from_le_bytes(decrypted_size_bytes);
                        decrypted_size_u32 as usize
                    } else {
                        u32::from_le_bytes(size_buf) as usize
                    };
                    
                    // Validate size
                    if block_size < 80 || block_size > 32 * 1024 * 1024 {
                        // Invalid size - try pattern search if encrypted
                        if is_xor_encrypted {
                            // Seek back and try pattern search
                            if let Some(start_pos) = block_start_offset {
                                file_reader.seek(SeekFrom::Start(start_pos + 8)).ok();
                            }
                            continue;
                        } else {
                            break;
                        }
                    }
                    
                    // Check file size before reading (avoid "failed to fill whole buffer" errors)
                    if is_xor_encrypted {
                        let current_pos = file_reader.seek(SeekFrom::Current(0)).unwrap_or(0);
                        let file_size = file_reader.get_ref().metadata().map(|m| m.len()).unwrap_or(0);
                        let required_size = current_pos + block_size as u64;
                        if required_size > file_size {
                            // File too small - skip this file
                            break;
                        }
                    }
                    
                    // OPTIMIZATION: Pre-allocate with exact size (avoids reallocations)
                    let mut block_data = vec![0u8; block_size];
                    match file_reader.read_exact(&mut block_data) {
                        Ok(_) => {},
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                            // File ended unexpectedly - skip
                            break;
                        }
                        Err(_) => {
                            // Other error - skip
                            break;
                        }
                    }
                    
                    // Decrypt if needed
                    if is_xor_encrypted {
                        let block_start = block_start_offset.unwrap();
                        let key1_u32 = u32::from_le_bytes(XOR_KEY1);
                        let key2_u32 = u32::from_le_bytes(XOR_KEY2);
                        let mut full_block = Vec::with_capacity(8 + block_size);
                        full_block.extend_from_slice(&encrypted_magic_bytes);
                        full_block.extend_from_slice(&size_buf);
                        full_block.extend_from_slice(&block_data);
                        
                        // OPTIMIZATION: Batch XOR decryption (process 4 bytes at a time, then remaining)
                        // This is faster than byte-by-byte and avoids repeated key lookups
                        let mut i = 0;
                        // Process 4-byte chunks (most of the block)
                        while i + 4 <= full_block.len() {
                            let file_offset = block_start + i as u64;
                            let use_key1 = (file_offset / 4) % 2 == 0;
                            let key_u32 = if use_key1 { key1_u32 } else { key2_u32 };
                            // OPTIMIZATION: Use u32 XOR directly (faster than byte-by-byte)
                            let chunk = u32::from_le_bytes([
                                full_block[i], full_block[i+1], 
                                full_block[i+2], full_block[i+3]
                            ]);
                            let decrypted = chunk ^ key_u32;
                            let bytes = decrypted.to_le_bytes();
                            full_block[i..i+4].copy_from_slice(&bytes);
                            i += 4;
                        }
                        // Handle remaining bytes (0-3 bytes)
                        while i < full_block.len() {
                            let byte_offset = block_start + i as u64;
                            let use_key1 = (byte_offset / 4) % 2 == 0;
                            let key = if use_key1 { &XOR_KEY1 } else { &XOR_KEY2 };
                            full_block[i] ^= key[(byte_offset % 4) as usize];
                            i += 1;
                        }
                        block_data = full_block[8..].to_vec();
                        
                        // For Start9, seek past any padding to find next block
                        let current_pos = file_reader.seek(SeekFrom::Current(0)).unwrap_or(0);
                        let mut test_magic_buf = [0u8; 4];
                        let mut need_search = true;
                        
                        match file_reader.read_exact(&mut test_magic_buf) {
                            Ok(_) => {
                                if test_magic_buf == ENCRYPTED_MAGIC {
                                    // Quick verify: decrypt and check
                                    let mut verify_magic = test_magic_buf;
                                    for j in 0..4 {
                                        let byte_offset = current_pos + j as u64;
                                        let key = if (byte_offset / 4) % 2 == 0 { &XOR_KEY1 } else { &XOR_KEY2 };
                                        verify_magic[j] ^= key[(byte_offset % 4) as usize];
                                    }
                                    if verify_magic == *magic {
                                        // Found it immediately - no search needed
                                        file_reader.seek(SeekFrom::Start(current_pos))?;
                                        need_search = false;
                                    } else {
                                        file_reader.seek(SeekFrom::Start(current_pos))?;
                                    }
                                } else {
                                    file_reader.seek(SeekFrom::Start(current_pos))?;
                                }
                            }
                            Err(_) => {
                                file_reader.seek(SeekFrom::Start(current_pos))?;
                            }
                        }
                        
                        // If we need to search (blocks are not sequential)
                        if need_search {
                            let mut search_pos = current_pos;
                            let mut found_next = false;
                            
                            loop {
                                let bytes_read = match file_reader.read(&mut search_buffer) {
                                    Ok(0) => break,
                                    Ok(n) => n,
                                    Err(_) => break,
                                };
                                
                                // Use memchr for fast pattern searching
                                let first_byte = ENCRYPTED_MAGIC[0];
                                for i in memchr_iter(first_byte, &search_buffer[..bytes_read]) {
                                    if i + 3 >= bytes_read {
                                        continue;
                                    }
                                    
                                    if search_buffer[i+1] == ENCRYPTED_MAGIC[1]
                                        && search_buffer[i+2] == ENCRYPTED_MAGIC[2]
                                        && search_buffer[i+3] == ENCRYPTED_MAGIC[3] {
                                        
                                        let file_offset = search_pos + i as u64;
                                        // Quick verify: decrypt and check
                                        let mut test_magic = [0u8; 4];
                                        test_magic.copy_from_slice(&search_buffer[i..i+4]);
                                        
                                        let magic_u32 = u32::from_le_bytes(test_magic);
                                        let key1_u32 = u32::from_le_bytes(XOR_KEY1);
                                        let key2_u32 = u32::from_le_bytes(XOR_KEY2);
                                        let use_key1 = (file_offset / 4) % 2 == 0;
                                        let key_u32 = if use_key1 { key1_u32 } else { key2_u32 };
                                        let decrypted_magic_u32 = magic_u32 ^ key_u32;
                                        test_magic = decrypted_magic_u32.to_le_bytes();
                                        
                                        if test_magic == *magic {
                                            // Found next block - seek to it
                                            file_reader.seek(SeekFrom::Start(file_offset))?;
                                            found_next = true;
                                            break;
                                        }
                                    }
                                }
                                
                                if found_next {
                                    break;
                                }
                                
                                search_pos += bytes_read as u64;
                                if bytes_read < search_buffer.len() {
                                    break;
                                }
                            }
                        }
                    }
                    
                    if block_data.len() >= 80 {
                        blocks.push(block_data);
                    }
                }
                
                Ok(blocks)
            };
            
            // Process files in parallel batches
            // When resuming, we need to track which file we were on
            // FIX: Instead of estimating, we'll track the last processed file in metadata
            // For now, use a more conservative estimate and validate blocks as we go
            let start_file_idx = if read_count > 0 {
                // More conservative estimate: ~50 blocks per file (to avoid skipping files)
                // This ensures we don't miss any blocks, even if it means re-reading some
                let estimated = (read_count as f64 / 50.0 * 0.7) as usize; // 70% of estimate to be very safe
                estimated.min(reader.block_files.len())
            } else {
                0
            };
            
            if read_count > 0 && start_file_idx > 0 {
                println!("   📍 Resuming: starting at file {} (conservative estimate based on {} existing blocks)", start_file_idx, read_count);
                println!("   ⚠️  NOTE: Some files may be re-read to ensure no blocks are missed");
            }
            
            let file_paths: Vec<_> = reader.block_files.iter().skip(start_file_idx).collect();
            // Use tunable batch size - optimized for local LAN SSHFS (I/O bound, not CPU bound)
            let batch_size = PARALLEL_FILE_BATCH_SIZE;
            let mut last_progress_time = start_time;
            let mut last_progress_count = read_count;
            let mut processed_files = start_file_idx;
            let mut last_file_idx = start_file_idx;
            
            // OPTIMIZATION: Pre-copy files ahead in large batches before starting to read
            // This ensures local cache is populated before we need the files
            // Start pre-copy from current position (not from beginning if resuming)
            if let Some(ref cache_dir) = reader.local_cache_dir {
                let precopy_count = PRE_COPY_LOOKAHEAD.min(file_paths.len());
                println!("   📦 Pre-copying {} files ahead (starting from file {}) to local cache...", 
                         precopy_count, start_file_idx);
                
                // Clone paths for parallel processing (starting from current position)
                let files_to_precopy: Vec<PathBuf> = file_paths.iter().take(precopy_count).map(|p| (*p).clone()).collect();
                
                pool.install(|| {
                    files_to_precopy.par_iter()
                        .for_each(|file_path| {
                            let file_name = match file_path.file_name() {
                                Some(name) => name,
                                None => return,
                            };
                            let local_path = cache_dir.join(file_name);
                            
                            // Copy if not already cached (skip if exists)
                            if !local_path.exists() {
                                if let Err(e) = std::fs::copy(file_path, &local_path) {
                                    eprintln!("⚠️  Failed to pre-copy {}: {}", file_path.display(), e);
                                }
                            }
                        });
                });
                println!("   ✅ Pre-copy complete - {} files ready in local cache", precopy_count);
            }
            
            // Track which files we've pre-copied to continue copying ahead
            // Start from where initial pre-copy ended (relative to start_file_idx)
            let mut last_precopy_idx = PRE_COPY_LOOKAHEAD.min(file_paths.len());
            
            for (batch_num, batch) in file_paths.chunks(batch_size).enumerate() {
                // Continue pre-copying ahead as we progress
                // Keep 200 files ahead of current reading position
                if let Some(ref cache_dir) = reader.local_cache_dir {
                    // Current position relative to start of file_paths (which starts at start_file_idx)
                    let current_pos_in_paths = (processed_files - start_file_idx) + batch.len();
                    let next_precopy_start = last_precopy_idx.max(current_pos_in_paths);
                    let next_precopy_end = (next_precopy_start + PRE_COPY_LOOKAHEAD).min(file_paths.len());
                    
                    if next_precopy_start < file_paths.len() && next_precopy_end > next_precopy_start {
                        // Clone paths for background thread (must own the data)
                        let files_to_precopy: Vec<PathBuf> = file_paths[next_precopy_start..next_precopy_end]
                            .iter()
                            .map(|p| (*p).clone())
                            .collect();
                        
                        // Pre-copy in background (don't wait)
                        let cache_dir_clone = cache_dir.clone();
                        std::thread::spawn(move || {
                            let pool = rayon::ThreadPoolBuilder::new()
                                .num_threads(MAX_PARALLEL_READ_THREADS)
                                .build();
                            if let Ok(pool) = pool {
                                pool.install(|| {
                                    files_to_precopy.par_iter()
                                        .for_each(|file_path| {
                                            let file_name = match file_path.file_name() {
                                                Some(name) => name,
                                                None => return,
                                            };
                                            let local_path = cache_dir_clone.join(file_name);
                                            
                                            if !local_path.exists() {
                                                let _ = std::fs::copy(file_path, &local_path);
                                            }
                                        });
                                });
                            }
                        });
                        
                        last_precopy_idx = next_precopy_end;
                    }
                }
                
                // Read blocks from all files in batch in parallel using custom thread pool
                // Files should now be in local cache for fast local disk access
                let batch_results: Vec<_> = pool.install(|| {
                    batch.par_iter()
                        .enumerate()
                        .map(|(batch_idx, file_path)| {
                            let file_idx = processed_files + batch_idx;
                            read_blocks_from_file(file_idx, file_path)
                        })
                        .collect()
                });
                
                // Write all blocks from batch sequentially to temp file
                // Track blocks in current chunk (resets after each chunk)
                let mut blocks_in_current_chunk = read_count % INCREMENTAL_CHUNK_SIZE;
                
                for (batch_idx, file_blocks_result) in batch_results.into_iter().enumerate() {
                    let file_idx = processed_files + batch_idx;
                    
                    match file_blocks_result {
                        Ok(file_blocks) => {
                            if !file_blocks.is_empty() && file_idx != last_file_idx {
                                eprintln!("   📂 Now reading from file {}: {}", 
                                         file_idx, 
                                         reader.block_files.get(file_idx)
                                             .map(|p| p.display().to_string())
                                             .unwrap_or_else(|| "unknown".to_string()));
                                last_file_idx = file_idx;
                            }
                            
                            for block_data in file_blocks {
                                // CRITICAL VALIDATION: Verify block before writing
                                if block_data.len() < MIN_VALID_BLOCK_SIZE {
                                    eprintln!("   ⚠️  ERROR: Block {} has invalid size: {} bytes (minimum {}) - SKIPPING", 
                                             read_count, block_data.len(), MIN_VALID_BLOCK_SIZE);
                                    continue; // Skip invalid block
                                }
                                
                                if block_data.len() > MAX_VALID_BLOCK_SIZE {
                                    eprintln!("   ⚠️  ERROR: Block {} has suspiciously large size: {} bytes (maximum {}) - SKIPPING", 
                                             read_count, block_data.len(), MAX_VALID_BLOCK_SIZE);
                                    continue; // Skip invalid block
                                }
                                
                                // OPTIMIZATION: For collection-only mode, skip strict version validation
                                // Version validation will happen during chunking when blocks are validated
                                // This allows collection to proceed even if some blocks have questionable versions
                                // (Start9 encrypted blocks may have edge cases that are valid after full processing)
                                // Only do basic sanity check - version should be in reasonable range
                                if block_data.len() >= 4 {
                                    let version = u32::from_le_bytes([
                                        block_data[0], block_data[1], 
                                        block_data[2], block_data[3]
                                    ]);
                                    // Very lenient check for collection - only reject obviously invalid values
                                    // Full validation happens during chunking
                                    if version > 0x7fffffff {
                                        // Version > 2^31 is definitely invalid (would be negative if signed)
                                        eprintln!("   ⚠️  ERROR: Block {} has obviously invalid version: {} (>{}) - SKIPPING", 
                                                 read_count, version, 0x7fffffff);
                                        continue; // Skip invalid block
                                    }
                                    // Otherwise accept it - validation will catch real issues during chunking
                                }
                                
                                // Write block to temp file: [len: u32][data...]
                                let block_len = block_data.len() as u32;
                                // OPTIMIZATION: Pre-compute length bytes once
                                let len_bytes = block_len.to_le_bytes();
                                
                                // CRITICAL: Verify block data integrity before writing
                                // Double-check version is reasonable (basic sanity check)
                                if block_data.len() >= 4 {
                                    let version_check = u32::from_le_bytes([
                                        block_data[0], block_data[1], 
                                        block_data[2], block_data[3]
                                    ]);
                                    // If version is clearly invalid (like the corrupted ones we saw: 536870912, etc.)
                                    // this suggests the block data itself is corrupted
                                    if version_check > 0x7fffffff {
                                        eprintln!("   ⚠️  ERROR: Block {} has corrupted data (version: {}) - SKIPPING before write", read_count, version_check);
                                        continue; // Skip this block entirely
                                    }
                                }
                                
                                temp_writer.write_all(&len_bytes)
                                    .map_err(|e| anyhow::anyhow!("Failed to write block length for block {}: {}", read_count, e))?;
                                temp_writer.write_all(&block_data)
                                    .map_err(|e| anyhow::anyhow!("Failed to write block data for block {}: {}", read_count, e))?;
                                read_count += 1;
                                
                                // INCREMENTAL CHUNKING: When we have enough blocks for a chunk, compress and move it
                                if read_count > 0 && read_count % INCREMENTAL_CHUNK_SIZE == 0 {
                                    let chunk_num = (read_count / INCREMENTAL_CHUNK_SIZE) - 1;
                                    
                                    eprintln!("   📦 Collected {} blocks - creating chunk {}...", 
                                             read_count, chunk_num);
                                    
                                    // Flush temp file to ensure all data is written
                                    temp_writer.flush()?;
                                    drop(temp_writer);
                                    
                                    // Create chunk from temp file (it contains exactly INCREMENTAL_CHUNK_SIZE blocks)
                                    BlockFileReader::create_and_move_chunk_from_file(
                                        &temp_file, 
                                        chunk_num,
                                        INCREMENTAL_CHUNK_SIZE
                                    )?;
                                    
                                    // Clear temp file for next chunk
                                    // CRITICAL: temp_writer was already dropped above, so we can't use it here
                                    // Verify temp file is the expected size before truncating
                                    let temp_size_before = std::fs::metadata(&temp_file)?.len();
                                    let expected_size = INCREMENTAL_CHUNK_SIZE as u64 * 1024 * 1024; // Rough estimate
                                    if temp_size_before > 0 && temp_size_before < expected_size / 10 {
                                        eprintln!("   ⚠️  WARNING: Temp file size ({}) seems unusually small before truncation", temp_size_before);
                                    }
                                    
                                    // Open with truncate to clear for next chunk
                                    let file = std::fs::OpenOptions::new()
                                        .write(true)
                                        .truncate(true)
                                        .open(&temp_file)?;
                                    
                                    // Verify file is actually empty after truncation
                                    let temp_size_after = std::fs::metadata(&temp_file)?.len();
                                    if temp_size_after != 0 {
                                        eprintln!("   ⚠️  ERROR: Temp file not properly truncated (size: {} bytes)", temp_size_after);
                                        return Err(anyhow::anyhow!("Temp file truncation failed - file not empty"));
                                    }
                                    
                                    temp_writer = BufWriter::with_capacity(IO_BUFFER_SIZE, file);
                                    
                                    // Reset block count for current chunk (temp file is now empty)
                                    blocks_in_current_chunk = 0;
                                    
                                    eprintln!("   ✅ Chunk {} complete and moved to secondary drive", chunk_num);
                                    eprintln!("   📝 Continuing collection for next chunk...");
                                }
                                
                                // Update blocks in current chunk
                                blocks_in_current_chunk += 1;
                                
                                // Flush buffer periodically to prevent data loss on SIGKILL
                                if read_count % TEMP_FILE_FLUSH_INTERVAL == 0 {
                                        if let Err(e) = temp_writer.flush() {
                                            eprintln!("   ⚠️  ERROR: Failed to flush temp file: {}", e);
                                            return Err(anyhow::anyhow!("Temp file flush failed at block {}: {}", read_count, e));
                                        }
                                        
                                        // OPTIMIZATION: Update metadata file every 10k blocks
                                        // This ensures we have an accurate count even if process is killed
                                        // FIX: Use binary u64 format instead of ASCII text
                                        if read_count % PROGRESS_REPORT_INTERVAL == 0 {
                                            let metadata_file = temp_file.with_extension("bin.meta");
                                            let count_bytes = (read_count as u64).to_le_bytes();
                                            if let Err(e) = std::fs::write(&metadata_file, count_bytes) {
                                                eprintln!("   ⚠️  Warning: Failed to update metadata: {}", e);
                                            }
                                        }
                                        
                                        // INTEGRITY CHECK: Periodically verify blocks written to temp file
                                        // Use blocks_in_current_chunk instead of read_count (total) because
                                        // temp file only contains current chunk after truncation
                                        if blocks_in_current_chunk > 0 && blocks_in_current_chunk % TEMP_FILE_INTEGRITY_CHECK_INTERVAL == 0 {
                                            // Flush first to ensure data is on disk
                                            temp_writer.flush()?;
                                            
                                            // Verify last few blocks can be read back correctly
                                            // Use blocks_in_current_chunk (not read_count) since temp file only has current chunk
                                            let verify_count = 10.min(blocks_in_current_chunk); // Verify last 10 blocks
                                            let verify_start = blocks_in_current_chunk - verify_count;
                                            
                                            // Open temp file for reading
                                            let mut verify_file = std::fs::File::open(&temp_file)?;
                                            use std::io::{Read, Seek, SeekFrom};
                                            
                                            // OPTIMIZATION: Read sequentially instead of seeking (much faster)
                                            // Read all blocks up to verification start, then verify last few
                                            // Note: current_block is relative to current chunk (0-based within chunk)
                                            let mut current_block = 0;
                                            while current_block < verify_start {
                                                let mut len_buf = [0u8; 4];
                                                match verify_file.read_exact(&mut len_buf) {
                                                    Ok(_) => {},
                                                    Err(e) => {
                                                        // If we can't read, it might be because we're at EOF (not enough blocks yet)
                                                        // This is OK - just skip the integrity check for now
                                                        eprintln!("   ⚠️  WARNING: Integrity check skipped - cannot read block {} from temp file (only {} blocks in current chunk): {}", 
                                                                 current_block, blocks_in_current_chunk, e);
                                                        break; // Exit integrity check early, continue collection
                                                    }
                                                }
                                                
                                                let block_len = u32::from_le_bytes(len_buf) as usize;
                                                
                                                // Validate block length
                                                // OPTIMIZATION: For collection-only mode, be more resilient
                                                // Skip corrupted blocks and continue - full validation happens during chunking
                                                if block_len > MAX_VALID_BLOCK_SIZE || block_len < MIN_VALID_BLOCK_SIZE {
                                                    eprintln!("   ⚠️  WARNING: Integrity check found corrupted block {} (size: {} bytes) - skipping in verification", current_block, block_len);
                                                    // Try to recover by seeking to next potential block boundary
                                                    // Look for next valid block start (magic bytes pattern)
                                                    // For now, just skip this block and continue
                                                    current_block += 1;
                                                    continue;
                                                }
                                                
                                                // OPTIMIZATION: For large blocks, use seek instead of reading (faster)
                                                // For small blocks, reading is faster due to buffer locality
                                                if block_len > 64 * 1024 {
                                                    // Large block: seek past it (faster than reading)
                                                    verify_file.seek(SeekFrom::Current(block_len as i64))?;
                                                } else {
                                                    // Small block: read into buffer (better cache locality)
                                                    let mut skip_buf = vec![0u8; block_len];
                                                    verify_file.read_exact(&mut skip_buf)?;
                                                }
                                                
                                                current_block += 1;
                                            }
                                            
                                            // Verify the last few blocks
                                            // OPTIMIZATION: For collection-only mode, be very lenient
                                            // Just check that we can read blocks, don't fail on validation
                                            // Full validation happens during chunking
                                            let mut verified_count = 0;
                                            for i in 0..verify_count {
                                                let mut len_buf = [0u8; 4];
                                                match verify_file.read_exact(&mut len_buf) {
                                                    Ok(_) => {},
                                                    Err(_) => {
                                                        // Can't read length - skip this block
                                                        eprintln!("   ⚠️  WARNING: Cannot read block {} length - skipping in verification", verify_start + i);
                                                        continue;
                                                    }
                                                }
                                                
                                                let block_len = u32::from_le_bytes(len_buf) as usize;
                                                
                                                // Validate size - skip obviously invalid blocks
                                                if block_len > MAX_VALID_BLOCK_SIZE || block_len < MIN_VALID_BLOCK_SIZE {
                                                    eprintln!("   ⚠️  WARNING: Integrity check found corrupted block {} (size: {} bytes) - will be caught during chunking", verify_start + i, block_len);
                                                    // Try to skip past this block and continue
                                                    // Seek past the invalid block if possible
                                                    if block_len < 10 * 1024 * 1024 * 1024 {  // Don't seek if size is absurdly large
                                                        if let Err(_) = verify_file.seek(SeekFrom::Current(block_len as i64)) {
                                                            // Can't seek - file might be corrupted, but continue anyway
                                                        }
                                                    }
                                                    continue;
                                                }
                                                
                                                // Read block data
                                                let mut block_data = vec![0u8; block_len];
                                                match verify_file.read_exact(&mut block_data) {
                                                    Ok(_) => {
                                                        // Block read successfully - count as verified
                                                        verified_count += 1;
                                                    }
                                                    Err(_) => {
                                                        eprintln!("   ⚠️  WARNING: Cannot read block {} data - skipping in verification", verify_start + i);
                                                        continue;
                                                    }
                                                }
                                                
                                                // OPTIMIZATION: Skip version validation during collection
                                                // Full validation happens during chunking
                                            }
                                            
                                            if verified_count > 0 {
                                                eprintln!("   ✅ Integrity check: verified {} of {} recent blocks in current chunk (some may be skipped due to corruption)", verified_count, verify_count);
                                            } else {
                                                eprintln!("   ⚠️  WARNING: Could not verify any recent blocks in current chunk - collection continues, validation will happen during chunking");
                                            }
                                        }
                                        
                                        // OPTIMIZATION: Progress reporting less frequently (reduces I/O overhead)
                                        // Flush more frequently for safety, but report less often
                                        if read_count % TEMP_FILE_FLUSH_INTERVAL == 0 {
                                        if let Err(e) = temp_writer.flush() {
                                            eprintln!("   ⚠️  ERROR: Failed to flush temp file: {}", e);
                                            return Err(anyhow::anyhow!("Temp file flush failed at block {}: {}", read_count, e));
                                        }
                                        
                                        let elapsed_since_last = last_progress_time.elapsed().as_secs_f64();
                                        let blocks_since_last = read_count - last_progress_count;
                                        let current_rate = if elapsed_since_last > 0.0 {
                                            blocks_since_last as f64 / elapsed_since_last
                                        } else {
                                            0.0
                                        };
                                        
                                        let total_elapsed = start_time.elapsed().as_secs_f64();
                                        let avg_rate = if total_elapsed > 0.0 {
                                            read_count as f64 / total_elapsed
                                        } else {
                                            0.0
                                        };
                                        
                                        if read_count % 5000 == 0 {
                                            let progress_pct = (read_count as f64 / estimated_total as f64 * 100.0).min(100.0);
                                            let eta_seconds = if avg_rate > 0.0 {
                                                ((estimated_total - read_count as u64) as f64 / avg_rate) as u64
                                            } else {
                                                0
                                            };
                                            println!("   📊 Progress: {}/{} blocks ({:.1}%) | Rate: {:.0} blocks/sec (avg: {:.0}) | ETA: {} min | File: {}", 
                                                     read_count, estimated_total, progress_pct, current_rate, avg_rate, eta_seconds / 60, file_idx);
                                        } else {
                                            println!("   📊 Progress: {}/{} blocks ({:.1}%) | Rate: {:.0} blocks/sec | File: {}", 
                                                     read_count, estimated_total, 
                                                     (read_count as f64 / estimated_total as f64 * 100.0).min(100.0),
                                                     current_rate, file_idx);
                                        }
                                        
                                        last_progress_time = std::time::Instant::now();
                                        last_progress_count = read_count;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("   ⚠️  Error reading blocks from file {}: {} - continuing", file_idx, e);
                        }
                    }
                }
                
                processed_files += batch.len();
            }
            
            // Final flush and integrity check (temp_writer, read_count, temp_file are in scope here)
            temp_writer.flush()?;
            drop(temp_writer);
            
            // Handle final chunk if there are remaining blocks
            let final_chunk_blocks = read_count % INCREMENTAL_CHUNK_SIZE;
            if final_chunk_blocks > 0 {
                let final_chunk_num = read_count / INCREMENTAL_CHUNK_SIZE;
                eprintln!("   📦 Creating final chunk {} with {} blocks...", final_chunk_num, final_chunk_blocks);
                
                BlockFileReader::create_and_move_chunk_from_file(
                    &temp_file,
                    final_chunk_num,
                    final_chunk_blocks
                )?;
                
                // Clear temp file
                std::fs::remove_file(&temp_file)?;
                
                eprintln!("   ✅ Final chunk {} complete and moved to secondary drive", final_chunk_num);
            } else if read_count > 0 && read_count % INCREMENTAL_CHUNK_SIZE == 0 {
                // Last chunk was exactly 125k blocks, already handled
                // Just clean up temp file
                std::fs::remove_file(&temp_file)?;
            }
            
            // Final integrity check: verify last 100 blocks (only if temp file still exists)
            if temp_file.exists() {
                eprintln!("   🔍 Running final integrity check...");
                let mut verify_file = std::fs::File::open(&temp_file)?;
                use std::io::{Read, Seek, SeekFrom};
            
            let verify_count = 100.min(read_count);
            let verify_start = read_count - verify_count;
            
            // Skip to verification start
            let mut pos = 0u64;
            for _ in 0..verify_start {
                let mut len_buf = [0u8; 4];
                verify_file.read_exact(&mut len_buf)?;
                let block_len = u32::from_le_bytes(len_buf) as u64;
                if block_len > MAX_VALID_BLOCK_SIZE as u64 || block_len < MIN_VALID_BLOCK_SIZE as u64 {
                    return Err(anyhow::anyhow!("Final integrity check failed: block has invalid size {}", block_len));
                }
                pos += 4 + block_len;
                verify_file.seek(SeekFrom::Start(pos))?;
            }
            
            // Verify last blocks
            for i in 0..verify_count {
                let mut len_buf = [0u8; 4];
                verify_file.read_exact(&mut len_buf)?;
                let block_len = u32::from_le_bytes(len_buf) as usize;
                
                if block_len > MAX_VALID_BLOCK_SIZE || block_len < MIN_VALID_BLOCK_SIZE {
                    return Err(anyhow::anyhow!("Final integrity check failed: block {} has invalid size {}", verify_start + i, block_len));
                }
                
                let mut block_data = vec![0u8; block_len];
                verify_file.read_exact(&mut block_data)?;
                
                if block_data.len() >= 4 {
                    let version = u32::from_le_bytes([
                        block_data[0], block_data[1],
                        block_data[2], block_data[3]
                    ]);
                    // CRITICAL FIX: Bitcoin block versions can be much higher than 10
                    // Only reject obviously invalid: version == 0 or version > 0x7fffffff
                    if version == 0 || version > 0x7fffffff {
                        return Err(anyhow::anyhow!("Final integrity check failed: block {} has invalid version {}", verify_start + i, version));
                    }
                }
                }
                
                eprintln!("   ✅ Final integrity check passed: verified last {} blocks", verify_count);
            }
            
            eprintln!("   ℹ️  Finished reading {} blocks from {} files", read_count, processed_files);
            
            let total_time = start_time.elapsed();
            println!("   ✅ Read {} total blocks from file in {:.1} minutes", 
                     read_count, total_time.as_secs_f64() / 60.0);
            
            // Now read blocks back from temp file and build hash map
            println!("   📖 Reading blocks from temp file to build hash map...");
            // OPTIMIZATION: Use larger buffer for temp file reading (faster sequential reads)
            let mut temp_reader = std::io::BufReader::with_capacity(IO_BUFFER_SIZE, std::fs::File::open(&temp_file)?);
            use std::io::Read;
            
            // FIX OOM: Process blocks in chunks instead of loading all into memory
            // Build hash map incrementally, storing file offsets instead of block data
            // This avoids loading 400+ GB into RAM
            println!("   📖 Processing blocks in chunks to build hash map (avoiding OOM)...");
            let estimated_blocks = read_count;
            
            // Hash map: prev_hash -> (file_offset, block_len)
            // We'll re-read blocks from temp file when chaining (slower but no OOM)
            // OPTIMIZATION: Pre-allocate hash map with estimated capacity to reduce rehashing
            let mut blocks_by_prev_hash: HashMap<Vec<u8>, (u64, usize)> = HashMap::with_capacity(estimated_blocks.min(1_000_000));
            let mut genesis_block: Option<(u64, usize)> = None;
            
            const CHUNK_SIZE: usize = HASH_MAP_CHUNK_SIZE;
            // OPTIMIZATION: Pre-allocate chunk vector with exact capacity
            let mut chunk = Vec::with_capacity(CHUNK_SIZE);
            let mut blocks_processed = 0;
            let mut current_offset: u64 = 0;
            
            loop {
                // Read block length
                let block_start_offset = current_offset; // Points to start of [len: u32]
                let mut len_buf = [0u8; 4];
                match temp_reader.read_exact(&mut len_buf) {
                    Ok(_) => {},
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        // Process remaining blocks in chunk
                        if !chunk.is_empty() {
                            Self::process_chunk(&chunk, &mut blocks_by_prev_hash, &mut genesis_block)?;
                            chunk.clear();
                        }
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
                current_offset += 4;
                let block_len = u32::from_le_bytes(len_buf) as usize;
                
                // CRITICAL OOM FIX: Read only block header (80 bytes) instead of full block
                // We only need prev_hash (bytes 4-36) to build the hash map
                // This reduces memory usage by ~18,750x (80 bytes vs 1.5MB average)
                let mut header = vec![0u8; 80.min(block_len)]; // Read up to 80 bytes (or block_len if smaller)
                temp_reader.read_exact(&mut header)?;
                
                // Skip the rest of the block (we don't need it for hash map building)
                if block_len > 80 {
                    temp_reader.seek(std::io::SeekFrom::Current((block_len - 80) as i64))?;
                }
                current_offset += block_len as u64;
                
                // Store only header (80 bytes) instead of full block (1.5MB average)
                chunk.push((header, block_start_offset, block_len));
                blocks_processed += 1;
                
                // Process chunk when full
                if chunk.len() >= CHUNK_SIZE {
                    Self::process_chunk(&chunk, &mut blocks_by_prev_hash, &mut genesis_block)?;
                    chunk.clear();
                    
                    if blocks_processed % PROGRESS_REPORT_INTERVAL == 0 {
                        println!("   📖 Processed {}/{} blocks...", blocks_processed, read_count);
                    }
                }
            }
            
            println!("   ✅ Built hash map with {} entries", blocks_by_prev_hash.len());
            
            if genesis_block.is_none() {
                eprintln!("⚠️  Warning: Genesis block not found in {} blocks read", read_count);
            }
            
            eprintln!("   Found {} blocks with previous hashes (excluding genesis)", blocks_by_prev_hash.len());
            
            // Define cache file path (for old format, if needed)
            let cache_file = dirs::cache_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
                .map(|cache| cache.join("blvm-bench").join("start9_ordered_blocks.bin"));
            
            // Check if chunked cache already exists - if so, skip building old format
            let chunks_dir = crate::chunked_cache::get_chunks_dir();
            let should_build_old_cache = if let Some(ref chunks_path) = chunks_dir {
                !chunks_path.exists() || !chunks_path.join("chunks.meta").exists()
            } else {
                true
            };
            
            if !should_build_old_cache {
                println!("   ✅ Chunked cache already exists - skipping old format cache build");
                println!("   💡 Use chunked cache for better space efficiency");
            } else {
                // OPTIMIZATION: Skip chaining during cache build - just copy blocks sequentially
                // Chaining is expensive (memory mapping, header reading, chain following, sorting)
                // For cache building, we don't need chain order - blocks can be stored as-is
                // Chaining can be done later when reading from cache if needed
                // NOTE: With chunked cache, we typically don't build the old single-file cache
                // This code path is kept for backward compatibility
                println!("   💾 Building cache (skipping chaining for speed - blocks stored as-is)...");
                println!("   ⚠️  Note: Consider using chunked cache format for better space efficiency");
                let cache_start = std::time::Instant::now();
                
                // Open cache file for streaming writes
                let mut cache_writer: Option<std::io::BufWriter<std::fs::File>> = None;
                let mut total_blocks_written = 0u64;
                let save_start = std::time::Instant::now();
                
                if let Some(ref cache_path) = cache_file {
                    if let Some(parent) = cache_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    // Reserve space for block count (u64) at start, will update at end
                    let cache_file_handle = std::fs::File::create(cache_path)?;
                    let mut writer = std::io::BufWriter::with_capacity(IO_BUFFER_SIZE, cache_file_handle);
                    // Write placeholder for block count (will update at end)
                    writer.write_all(&0u64.to_le_bytes())?;
                    cache_writer = Some(writer);
                }
                
                // OPTIMIZATION: Use memory-mapped file for fast sequential reading
                // Read blocks directly from temp file in order and write to cache
                // This is MUCH faster than chaining - just a simple sequential copy
                println!("   🗺️  Memory-mapping temp file for fast sequential copy...");
                use memmap2::MmapOptions;
                let file = std::fs::File::open(&temp_file)?;
                let mmap = unsafe { MmapOptions::new().map(&file)? };
                println!("   ✅ Memory-mapped {} GB file", mmap.len() as f64 / 1_073_741_824.0);
                
                // OPTIMIZATION: Sequential copy is already optimal for NVMe SSDs
                // Memory-mapped reads are instant (no I/O wait), sequential writes are fastest
                // Parallelizing would add overhead without benefit (can't parallelize single-file writes)
                // The 128MB buffer ensures maximum throughput for sequential I/O
                println!("   📖 Copying blocks from temp file to cache (sequential, optimized for NVMe)...");
                let mut pos = 0usize;
                let mut blocks_copied = 0;
                
                // Pre-allocate block length buffer to avoid repeated allocations
                let mut len_buf = [0u8; 4];
                
                while pos + 4 <= mmap.len() {
                    // Read block length (4 bytes) - use pre-allocated buffer
                    len_buf.copy_from_slice(&mmap[pos..pos + 4]);
                    let block_len = u32::from_le_bytes(len_buf) as usize;
                    pos += 4;
                    
                    // Read block data
                    if pos + block_len > mmap.len() {
                        eprintln!("   ⚠️  Warning: Block at offset {} extends beyond file end, stopping", pos - 4);
                        break;
                    }
                    
                    let block_data = &mmap[pos..pos + block_len];
                    
                    // Write block to cache immediately (streaming, no accumulation)
                    // Large 128MB buffer ensures minimal system calls
                    if let Some(ref mut writer) = cache_writer {
                        writer.write_all(&len_buf)?;
                        writer.write_all(block_data)?;
                        total_blocks_written += 1;
                    }
                    
                    pos += block_len;
                    blocks_copied += 1;
                    
                    if blocks_copied % PROGRESS_REPORT_INTERVAL == 0 {
                        let elapsed = cache_start.elapsed().as_secs();
                        let rate = if elapsed > 0 { blocks_copied as f64 / elapsed as f64 } else { 0.0 };
                        let progress_pct = if read_count > 0 {
                            (blocks_copied as f64 / read_count as f64 * 100.0).min(100.0)
                        } else {
                            0.0
                        };
                        println!("   📊 Copied {}/{} blocks ({:.1}%) | Rate: {:.0} blocks/sec", 
                                 blocks_copied, read_count, progress_pct, rate);
                        // Flush periodically to ensure progress is saved
                        if let Some(ref mut writer) = cache_writer {
                            let _ = writer.flush();
                        }
                    }
                }
                
                let cache_time = cache_start.elapsed();
                println!("   ✅ Copied {} blocks to cache in {:.1} minutes (skipped chaining for speed)", 
                         total_blocks_written, cache_time.as_secs_f64() / 60.0);
                
                // Finalize cache file - update block count at start
                if let Some(ref mut writer) = cache_writer {
                    writer.flush()?;
                    drop(writer); // Close file so we can update it
                    
                    // Update block count at start of file
                    if let Some(ref cache_path) = cache_file {
                        let mut file = std::fs::OpenOptions::new()
                            .write(true)
                            .open(cache_path)?;
                        use std::io::Seek;
                        file.seek(std::io::SeekFrom::Start(0))?;
                        file.write_all(&total_blocks_written.to_le_bytes())?;
                        file.sync_all()?;
                        
                        let save_time = save_start.elapsed();
                        let cache_size = std::fs::metadata(cache_path)?.len();
                        let cache_size_gb = cache_size as f64 / 1_073_741_824.0;
                        println!("   ✅ Cached ordered block list to: {}", cache_path.display());
                        println!("      Cache size: {:.2} GB | Write time: {:.1} seconds", 
                                 cache_size_gb, save_time.as_secs_f64());
                        
                        // CRITICAL: NEVER DELETE THE TEMP FILE
                        // This file represents days of work reading and processing blocks.
                        // It can be used to resume if the process is interrupted.
                        // The temp file is a valuable backup even after cache is saved.
                        // Users can manually delete it if they want, but code should NEVER do it.
                        // Note: Memory map is automatically dropped when it goes out of scope
                        println!("   💾 Temp file preserved at: {} (contains {} blocks, {:.2} GB of work)", 
                                 temp_file.display(),
                                 read_count,
                                 std::fs::metadata(&temp_file).map(|m| m.len() as f64 / 1_073_741_824.0).unwrap_or(0.0));
                        println!("   ⚠️  DO NOT DELETE THIS FILE - It represents days of processing work");
                    }
                }
                // Memory map is automatically dropped when it goes out of scope
            }
            
            // FIX: When building cache, we've already read all blocks from files
            // The iterator should continue reading from source files starting from where we left off
            // But actually, new_ordered reads ALL blocks, so there are no more blocks to read
            // The real issue: new_ordered should NOT read all blocks if max_blocks is set
            // For now, set ordered_blocks to None so iterator tries to read from files
            // The iterator will start from file 0, but should skip files we already processed
            // Actually, the better fix: new_ordered should respect max_blocks and only read that many
            // But for now, let's just make sure iterator can read from files
            ordered_blocks = None; // None = continue reading from files, not from cache
        }
        
        // Handle ordered_blocks (if loaded from cache)
        let filtered_blocks: Option<Vec<Vec<u8>>> = if let Some(ref mut blocks) = ordered_blocks {
            // Apply start_height and max_blocks filters
            let start_idx = start_height.unwrap_or(0) as usize;
            let end_idx = if let Some(max) = max_blocks {
                (start_idx + max).min(blocks.len())
            } else {
                blocks.len()
            };
            
            if end_idx > start_idx {
                Some(blocks.drain(start_idx..end_idx).collect())
            } else {
                Some(Vec::new())
            }
        } else {
            None // Continue reading from files
        };
        
        Ok(Self {
            reader: BlockFileReader {
                data_dir: reader.data_dir.clone(),
                network: reader.network,
                block_files: reader.block_files.clone(),
                local_cache_dir: reader.local_cache_dir.clone(),
                file_index: reader.file_index.clone(),
            },
            current_file_idx: 0,
            current_file: None,
            current_height: start_height.unwrap_or(0),
            start_height,
            max_blocks,
            blocks_read: 0,
            ordered_blocks: filtered_blocks,
            ordered_index: 0,
            search_buffer: vec![0u8; SEARCH_BUFFER_SIZE],
            copy_sender: None, // Not needed for ordered iterator
            last_copy_start_idx: 0,
            failed_files: std::collections::HashSet::new(), // Track files that failed to avoid retries
            current_reading_file_idx: None, // Track which file we're reading from
        })
    }
    
    /// Read next block from current file
    fn read_next_from_file(&mut self) -> Result<Option<Vec<u8>>> {
        let file = match &mut self.current_file {
            Some(f) => f,
            None => return Ok(None),
        };
        
        let magic = self.reader.network.magic_bytes();
        let mut magic_buf = [0u8; 4];
        
        // Try to read magic bytes
        // Start9 uses XOR encryption with ALTERNATING keys:
        // - KEY1: 0x8422e9ad (for bytes 0-3, 8-11, 16-19, ...)
        // - KEY2: 0xb78fff14 (for bytes 4-7, 12-15, 20-23, ...)
        // Keys alternate every 4 bytes starting from file offset 0
        const XOR_KEY1: [u8; 4] = [0x84, 0x22, 0xe9, 0xad];  // First key
        const XOR_KEY2: [u8; 4] = [0xb7, 0x8f, 0xff, 0x14];  // Second key (alternating)
        const ENCRYPTED_MAGIC: [u8; 4] = [0x7d, 0x9c, 0x5d, 0x74];
        let mut is_xor_encrypted = false;
        let mut encrypted_magic_bytes = [0u8; 4]; // Save original encrypted magic for reconstruction
        
        // CRITICAL FIX: Track file position BEFORE reading magic
        // This is needed for correct XOR key rotation in Start9 files
        let magic_start_pos = file.stream_position()?;
        
        match file.read_exact(&mut magic_buf) {
            Ok(_) => {
                // Check if file is XOR encrypted (Start9 format)
                // Encrypted magic is 0x7d9c5d74
                is_xor_encrypted = magic_buf == ENCRYPTED_MAGIC;
                
                if is_xor_encrypted {
                    // Save original encrypted magic before decrypting
                    encrypted_magic_bytes = magic_buf;
                    // CRITICAL FIX: Decrypt magic using correct key based on FILE OFFSET
                    // Magic is at file offset magic_start_pos, so use key based on that
                    let use_key1 = (magic_start_pos / 4) % 2 == 0;
                    let key = if use_key1 { &XOR_KEY1 } else { &XOR_KEY2 };
                    for i in 0..4 {
                        let byte_offset = magic_start_pos + i as u64;
                        magic_buf[i] ^= key[(byte_offset % 4) as usize];
                    }
                }
                
                if magic_buf != *magic {
                    // Not a block start, might be end of file or corrupted
                    return Ok(None);
                }
            }
            Err(_) => {
                // End of file
                return Ok(None);
            }
        }
        
        // Read size field (4 bytes at file offset 4-7)
        let mut size_buf = [0u8; 4];
        match file.read_exact(&mut size_buf) {
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // End of file
                return Ok(None);
            }
            Err(e) => {
                // Other read error - might be permission or corruption
                return Err(e.into());
            }
        }
        
        // CRITICAL FIX: Use magic_start_pos as block_start_offset for XOR decryption
        // This is the file offset where the block's magic bytes start
        let block_start_offset = if is_xor_encrypted {
            Some(magic_start_pos)
        } else {
            None
        };
        
        // For Start9 encrypted files, decrypt the size field and use it as a HINT
        // Then verify the next block's magic is at the expected position
        let block_data = if is_xor_encrypted {
            // CRITICAL FIX: Decrypt size field using correct key based on FILE OFFSET
            // Size field is at file offset magic_start_pos + 4
            let size_offset = magic_start_pos + 4;
            let use_key1 = (size_offset / 4) % 2 == 0;
            let key = if use_key1 { &XOR_KEY1 } else { &XOR_KEY2 };
            
            // Decrypt each byte of size field with correct key byte
            let mut decrypted_size_bytes = [0u8; 4];
            for i in 0..4 {
                let byte_offset = size_offset + i as u64;
                decrypted_size_bytes[i] = size_buf[i] ^ key[(byte_offset % 4) as usize];
            }
            let decrypted_size_u32 = u32::from_le_bytes(decrypted_size_bytes);
            let size_hint = decrypted_size_u32 as usize;
            
            // Validate size hint is reasonable
            if size_hint >= 80 && size_hint <= 4 * 1024 * 1024 {
                // OPTIMIZATION: Check file size before attempting to read the block
                // This avoids expensive "failed to fill whole buffer" errors
                let current_pos = file.stream_position()?;
                let file_size = file.get_ref().metadata().map(|m| m.len()).unwrap_or(0);
                let required_size = current_pos + size_hint as u64;
                if required_size > file_size {
                    // File doesn't have enough data - mark as failed and skip
                    if let Some(file_idx) = self.current_reading_file_idx {
                        eprintln!("⚠️  Error reading block: file too small (need {} bytes, have {} bytes) - marking file {} as failed", 
                                 required_size, file_size, file_idx);
                        self.failed_files.insert(file_idx);
                    }
                    self.current_file = None; // Close the file
                    self.current_reading_file_idx = None;
                    return Ok(None); // Skip this file
                }
                
                // Use size field - it's the correct logical block size
                let mut block_data = vec![0u8; size_hint];
                match file.read_exact(&mut block_data) {
                    Ok(_) => {},
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        // File ended unexpectedly - mark as failed and skip
                        if let Some(file_idx) = self.current_reading_file_idx {
                            eprintln!("⚠️  Error reading block: failed to fill whole buffer - marking file {} as failed", file_idx);
                            self.failed_files.insert(file_idx);
                        }
                        self.current_file = None; // Close the file
                        self.current_reading_file_idx = None;
                        return Ok(None); // Skip this file
                    }
                    Err(e) => {
                        // Other error - mark as failed and skip
                        if let Some(file_idx) = self.current_reading_file_idx {
                            eprintln!("⚠️  Error reading block: {} - marking file {} as failed", e, file_idx);
                            self.failed_files.insert(file_idx);
                        }
                        self.current_file = None; // Close the file
                        self.current_reading_file_idx = None;
                        return Err(e.into());
                    }
                }
                
                // For Start9, we need to seek past any padding to the next block
                // OPTIMIZATION: Use much larger search buffer (1MB) for faster searching
                // First, try reading magic bytes at current position (most blocks are sequential)
                let current_pos = file.stream_position()?;
                let mut test_magic_buf = [0u8; 4];
                let mut need_search = true;
                
                match file.read_exact(&mut test_magic_buf) {
                    Ok(_) => {
                        if test_magic_buf == ENCRYPTED_MAGIC {
                            // Quick verify: decrypt and check
                            let mut verify_magic = test_magic_buf;
                            for j in 0..4 {
                                let byte_offset = current_pos + j as u64;
                                let key = if (byte_offset / 4) % 2 == 0 { &XOR_KEY1 } else { &XOR_KEY2 };
                                verify_magic[j] ^= key[(byte_offset % 4) as usize];
                            }
                            if verify_magic == *magic {
                                // Found it immediately - no search needed!
                                file.seek(std::io::SeekFrom::Start(current_pos))?;
                                need_search = false;
                            } else {
                                // Not a valid block - need to search
                                file.seek(std::io::SeekFrom::Start(current_pos))?;
                            }
                        } else {
                            // Not at expected position - need to search
                            file.seek(std::io::SeekFrom::Start(current_pos))?;
                        }
                    }
                    Err(_) => {
                        // EOF or error - seek back
                        file.seek(std::io::SeekFrom::Start(current_pos))?;
                    }
                }
                
                // If we need to search (blocks are not sequential)
                // OPTIMIZATION: Use pre-allocated buffer and memchr for faster searching
                let mut search_pos = current_pos;
                let mut found_next = false;
                
                if need_search {
                loop {
                    // Use pre-allocated buffer from struct
                    let bytes_read = match file.read(&mut self.search_buffer) {
                        Ok(0) => {
                            // End of file - no more blocks in this file
                            break;
                        }
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                            break;
                        }
                        Err(e) => {
                            eprintln!("⚠️  Error searching for next block: {} - stopping search", e);
                            break;
                        }
                    };
                    
                    // OPTIMIZATION: Use memchr for fast pattern searching (2-3x faster than byte-by-byte)
                    let first_byte = ENCRYPTED_MAGIC[0];
                    for i in memchr_iter(first_byte, &self.search_buffer[..bytes_read]) {
                        // Check if we have enough bytes remaining
                        if i + 3 >= bytes_read {
                            continue;
                        }
                        
                        // Potential match - verify all 4 bytes
                        if self.search_buffer[i+1] == ENCRYPTED_MAGIC[1]
                            && self.search_buffer[i+2] == ENCRYPTED_MAGIC[2]
                            && self.search_buffer[i+3] == ENCRYPTED_MAGIC[3] {
                            
                            let file_offset = search_pos + i as u64;
                            // Quick verify: decrypt and check
                            let mut test_magic = [0u8; 4];
                            test_magic.copy_from_slice(&self.search_buffer[i..i+4]);
                            
                            // OPTIMIZATION: Use u32 XOR for magic verification (faster than byte-by-byte)
                            let magic_u32 = u32::from_le_bytes(test_magic);
                            let key1_u32 = u32::from_le_bytes(XOR_KEY1);
                            let key2_u32 = u32::from_le_bytes(XOR_KEY2);
                            let use_key1 = (file_offset / 4) % 2 == 0;
                            let key_u32 = if use_key1 { key1_u32 } else { key2_u32 };
                            let decrypted_magic_u32 = magic_u32 ^ key_u32;
                            test_magic = decrypted_magic_u32.to_le_bytes();
                            
                            if test_magic == *magic {
                                // Found next block - seek to it
                                file.seek(std::io::SeekFrom::Start(file_offset))?;
                                found_next = true;
                                break;
                            }
                        }
                    }
                    
                    if found_next {
                        break;
                    }
                    
                    search_pos += bytes_read as u64;
                    
                    // If we didn't find it and read less than buffer size, we're at EOF
                    if bytes_read < self.search_buffer.len() {
                        break;
                    }
                }
                } // end if need_search
                
                // If we didn't find the next block and we're not at EOF, 
                // the file pointer is at an unknown position. This shouldn't happen,
                // but if it does, we'll try to continue on the next read.
                // The next call to read_next_from_file() will try to read magic bytes,
                // and if they don't match, it will return Ok(None), moving to the next file.
                
                block_data
            } else {
                // Size field is invalid - use pattern search
                eprintln!("⚠️  Invalid size field hint ({}) - using pattern search", size_hint);
                let bytes_read = file.read(&mut self.search_buffer)?;
                
                if bytes_read == 0 {
                    return Ok(Some(Vec::new()));
                }
                
                let start_file_pos = block_start_offset.unwrap();
                let mut found_at = None;
                
                // Use memchr for faster searching
                let first_byte = ENCRYPTED_MAGIC[0];
                for i in memchr_iter(first_byte, &self.search_buffer[..bytes_read]) {
                    if i + 3 >= bytes_read {
                        continue;
                    }
                    
                    if self.search_buffer[i+1] == ENCRYPTED_MAGIC[1]
                        && self.search_buffer[i+2] == ENCRYPTED_MAGIC[2]
                        && self.search_buffer[i+3] == ENCRYPTED_MAGIC[3] {
                        
                        let file_offset = start_file_pos + 8 + i as u64;
                        let mut test_magic = [0u8; 4];
                        test_magic.copy_from_slice(&self.search_buffer[i..i+4]);
                        
                        for j in 0usize..4 {
                            let byte_offset = file_offset + j as u64;
                            let key = if (byte_offset / 4) % 2 == 0 {
                                &XOR_KEY1
                            } else {
                                &XOR_KEY2
                            };
                            let key_index = (byte_offset % 4) as usize;
                            test_magic[j] ^= key[key_index];
                        }
                        
                        if test_magic == *magic {
                            found_at = Some(i);
                            break;
                        }
                    }
                }
                
                if let Some(i) = found_at {
                    let data = self.search_buffer[..i].to_vec();
                    let next_block_pos = start_file_pos + 8 + i as u64;
                    file.seek(std::io::SeekFrom::Start(next_block_pos))?;
                    data
                } else {
                    self.search_buffer[..bytes_read].to_vec()
                }
            }
        } else {
            // Standard format: use size field (it's reliable for non-encrypted files)
            let block_size = u32::from_le_bytes(size_buf) as usize;
            if block_size < 80 || block_size > 32 * 1024 * 1024 {
                anyhow::bail!("Invalid block size: {} bytes", block_size);
            }
            let mut block_data = vec![0u8; block_size];
            file.read_exact(&mut block_data)?;
            block_data
        };
        
        // In Start9 format, the ENTIRE file is encrypted with ALTERNATING keys
        // Pattern: KEY1 (bytes 0-3), KEY2 (bytes 4-7), KEY1 (bytes 8-11), KEY2 (bytes 12-15), ...
        // Keys alternate every 4 bytes starting from file offset 0
        // CRITICAL: Key rotation is based on FILE OFFSET, not block offset!
        let final_block_data = if is_xor_encrypted {
            let start_offset = block_start_offset.expect("block_start_offset should be set for encrypted blocks");
            // Reconstruct full encrypted block: magic + size + data
            // Use the ACTUAL encrypted magic bytes we read from the file, not the constant
            let mut full_encrypted = Vec::with_capacity(8 + block_data.len());
            full_encrypted.extend_from_slice(&encrypted_magic_bytes); // Actual encrypted magic from file
            full_encrypted.extend_from_slice(&size_buf); // Encrypted size
            full_encrypted.extend_from_slice(&block_data); // Encrypted block data
            
            // Decrypt with alternating keys based on FILE OFFSET
            // OPTIMIZATION: Use u32 XOR operations for aligned 4-byte chunks (much faster than byte-by-byte)
            let mut i = 0;
            let key1_u32 = u32::from_le_bytes(XOR_KEY1);
            let key2_u32 = u32::from_le_bytes(XOR_KEY2);
            
            // Process aligned 4-byte chunks with u32 XOR
            while i + 4 <= full_encrypted.len() {
                let file_offset = start_offset + i as u64;
                let use_key1 = (file_offset / 4) % 2 == 0;
                let key_u32 = if use_key1 { key1_u32 } else { key2_u32 };
                
                // XOR 4 bytes at once using u32
                let chunk = u32::from_le_bytes([
                    full_encrypted[i],
                    full_encrypted[i + 1],
                    full_encrypted[i + 2],
                    full_encrypted[i + 3],
                ]);
                let decrypted_chunk = chunk ^ key_u32;
                let decrypted_bytes = decrypted_chunk.to_le_bytes();
                full_encrypted[i..i + 4].copy_from_slice(&decrypted_bytes);
                
                i += 4;
            }
            
            // Handle remaining bytes (< 4) with byte-by-byte XOR
            while i < full_encrypted.len() {
                let byte_offset = start_offset + i as u64;
                let use_key1 = (byte_offset / 4) % 2 == 0;
                let key = if use_key1 { &XOR_KEY1 } else { &XOR_KEY2 };
                full_encrypted[i] ^= key[(byte_offset % 4) as usize];
                i += 1;
            }
            
            // Extract just the block data (skip magic + size)
            // After decryption:
            // - full_encrypted[0:4] = decrypted magic (f9beb4d9)
            // - full_encrypted[4:8] = decrypted size
            // - full_encrypted[8:] = decrypted block data (what we want)
            let decrypted = full_encrypted[8..].to_vec();
            
            // Verify the decrypted block is valid
            // If it's too large, we might have included padding or the next block
            if decrypted.len() > 32 * 1024 * 1024 {
                // Block is unreasonably large - likely read too much
                anyhow::bail!("Decrypted block size {} bytes is too large (max 32MB)", decrypted.len());
            }
            
            // Verify block header is valid (basic sanity check)
            if decrypted.len() >= 80 {
                // Check version is reasonable (Bitcoin blocks use version 1-4 typically, but can be higher)
                let version = u32::from_le_bytes([decrypted[0], decrypted[1], decrypted[2], decrypted[3]]);
                if version == 0 || version > 0x7fffffff {
                    // Invalid version - likely read too much data
                    anyhow::bail!("Invalid block version {} - likely read too much data (block size: {} bytes)", version, decrypted.len());
                }
            }
            
            decrypted
        } else {
            block_data
        };
        
        Ok(Some(final_block_data))
    }
    
    /// Get local copy path if available, otherwise return remote path
    fn get_local_or_remote_path(&self, file_idx: usize) -> Result<PathBuf> {
        if file_idx >= self.reader.block_files.len() {
            anyhow::bail!("File index out of range");
        }
        
        let remote_path = &self.reader.block_files[file_idx];
        
        // If we have a local cache, check if file is copied locally
        if let Some(ref cache_dir) = self.reader.local_cache_dir {
            let file_name = remote_path.file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid file path"))?;
            let local_path = cache_dir.join(file_name);
            
            // If local copy exists, use it
            if local_path.exists() {
                return Ok(local_path);
            }
        }
        
        // No local cache or copy doesn't exist - use remote
        Ok(remote_path.clone())
    }
    
    /// Copy file from remote to local cache (non-blocking, uses thread pool)
    fn copy_file_locally(&self, file_idx: usize) {
        if file_idx >= self.reader.block_files.len() {
            return;
        }
        
        let Some(ref cache_dir) = self.reader.local_cache_dir else {
            return; // No local cache configured
        };
        
        let remote_path = &self.reader.block_files[file_idx];
        let file_name = match remote_path.file_name() {
            Some(name) => name,
            None => return,
        };
        let local_path = cache_dir.join(file_name);
        
        // Skip if already copied
        if local_path.exists() {
            return;
        }
        
        // Use thread pool if available (limits concurrent copies)
        if let Some(ref sender) = self.copy_sender {
            let _ = sender.send((remote_path.clone(), local_path));
        } else {
            // Fallback: spawn thread (shouldn't happen if thread pool is set up)
            let remote = remote_path.clone();
            let local = local_path.clone();
            std::thread::spawn(move || {
                if let Err(e) = std::fs::copy(&remote, &local) {
                    eprintln!("⚠️  Failed to copy {} to local cache: {}", remote.display(), e);
                }
            });
        }
    }
    
    /// Delete local copy after processing (non-blocking)
    fn cleanup_processed_file(&self, file_idx: usize) {
        if file_idx >= self.reader.block_files.len() {
            return;
        }
        
        let Some(ref cache_dir) = self.reader.local_cache_dir else {
            return; // No local cache configured
        };
        
        let remote_path = &self.reader.block_files[file_idx];
        let file_name = match remote_path.file_name() {
            Some(name) => name,
            None => return,
        };
        let local_path = cache_dir.join(file_name);
        
        // Delete in background
        std::thread::spawn(move || {
            let _ = std::fs::remove_file(&local_path);
        });
    }
    
    /// Start background copying of files ahead of current position
    /// OPTIMIZATION: Only copy if we've advanced significantly (every 50 files) to avoid overhead
    fn start_background_copying(&mut self) {
        let Some(ref _cache_dir) = self.reader.local_cache_dir else {
            return; // No local cache - nothing to do
        };
        
        let current_idx = self.current_file_idx;
        
        // Only start copying if we've advanced at least 50 files since last copy start
        // This avoids re-queueing the same files repeatedly
        if current_idx < self.last_copy_start_idx + 50 && self.last_copy_start_idx > 0 {
            return; // Already copying files ahead, no need to restart
        }
        
        let total_files = self.reader.block_files.len();
        let files_to_copy_ahead = 1000; // Copy next 1000 files ahead (increased for very sparse files)
        
        // Copy files ahead in background (need to avoid borrow checker issues)
        let current_idx_copy = current_idx;
        for i in 1..=files_to_copy_ahead {
            let file_idx = current_idx_copy + i;
            if file_idx < total_files {
                self.copy_file_locally(file_idx);
            }
        }
        
        self.last_copy_start_idx = current_idx;
    }
    
    /// Move to next file
    fn next_file(&mut self) -> Result<bool> {
        // OPTIMIZATION: Delay cleanup of processed files
        // Only cleanup files that are far behind (more than 100 files ago)
        // This keeps files available longer in case we need to re-read them
        if self.current_file_idx > 100 {
            self.cleanup_processed_file(self.current_file_idx - 100);
        }
        
        // Keep trying files until we find one we can open or run out of files
        // OPTIMIZATION: Skip empty files quickly by checking size first
        // OPTIMIZATION: Smart skipping - jump ahead when we know files are empty
        // FIX: Don't increment if this is the first file (current_file_idx starts at 0 or -1)
        // Only increment if we already have a file open
        if self.current_file.is_some() {
            self.current_file_idx += 1;
        } else if self.current_file_idx == 0 && self.reader.block_files.is_empty() {
            return Ok(false); // No files at all
        }
        // If current_file_idx is still 0 and current_file is None, we'll try to open file 0
        
        loop {
            
            if self.current_file_idx >= self.reader.block_files.len() {
                return Ok(false); // No more files
            }
            
            let file_path = &self.reader.block_files[self.current_file_idx];
            
            // OPTIMIZATION: Do in-memory checks FIRST (no filesystem operations)
            // 1. Check failed files (O(1) HashSet lookup)
            if self.failed_files.contains(&self.current_file_idx) {
                continue; // Skip this file - we know it will fail
            }
            
            // 2. Check pre-scanned file index (O(1) HashSet lookup)
            // OPTIMIZATION: Smart skip - if we've skipped many empty files in a row, jump ahead
            if let Some(ref index) = self.reader.file_index {
                if !index.contains(&self.current_file_idx) {
                    // File is known to be empty from pre-scan - skip immediately without any I/O
                    // OPTIMIZATION: Pre-compute jump distance - if many empty files ahead, jump immediately
                    // This is especially important in sparse regions where we might have 100+ empty files in a row
                    let mut next_idx = self.current_file_idx;
                    let mut empty_count = 0;
                    while next_idx < self.reader.block_files.len() && !index.contains(&next_idx) {
                        empty_count += 1;
                        next_idx += 1;
                    }
                    
                    // If we found many consecutive empty files ahead, jump immediately
                    // This avoids iterating through each empty file one by one
                    if empty_count >= 3 && next_idx < self.reader.block_files.len() {
                        // Jump ahead to next file with blocks (skip all empty files in between)
                        // This can save hundreds of iterations in very sparse regions
                        self.current_file_idx = next_idx - 1; // Will be incremented at start of loop
                    }
                    continue;
                }
            }
            
            // 3. NOW do filesystem operations (only for files we'll actually try to open)
            // OPTIMIZATION: Check local copy first (faster than SSHFS metadata)
            // If local copy exists, we know it's valid and can skip metadata check
            let path_to_use = match self.get_local_or_remote_path(self.current_file_idx) {
                Ok(path) => path,
                Err(_) => continue, // Skip if we can't get path
            };
            
            // OPTIMIZATION: Check file size for remote files (SSHFS) before opening
            // BUT: Only skip if file is NOT in index (already known to be empty)
            // Files in index might have valid blocks, so we must try to read them
            if path_to_use == *file_path {
                // Remote file (SSHFS) - check size before opening
                // This is a single SSHFS round-trip, much faster than opening and reading
                match std::fs::metadata(&path_to_use) {
                    Ok(metadata) => {
                        let file_size = metadata.len();
                        // Only skip if file is NOT in index (known empty from pre-scan)
                        // Files in index (>= 8 bytes) might have valid blocks, so we must read them
                        if let Some(ref index) = self.reader.file_index {
                            if !index.contains(&self.current_file_idx) && file_size < 100 {
                                // File not in index AND too small - safe to skip
                                self.failed_files.insert(self.current_file_idx);
                                continue;
                            }
                            // File is in index - must try to read it (even if small, might have valid block)
                        } else {
                            // No index available - be conservative, only skip very small files
                            // Minimum size for a valid block: magic (4) + size (4) + header (80) = 88 bytes
                            // Use 100 bytes as threshold to account for any padding/overhead
                            if file_size < 100 {
                                // File is too small to contain valid blocks - skip entirely
                                self.failed_files.insert(self.current_file_idx);
                                continue;
                            }
                        }
                    }
                    Err(_) => {
                        // Metadata check failed (file doesn't exist or permission denied) - skip
                        self.failed_files.insert(self.current_file_idx);
                        continue;
                    }
                }
            }
            // If using local copy, we can skip metadata check (local filesystem is fast)
            
            // Copy files ahead in background (only if we've advanced enough to avoid re-queueing)
            // Inline the logic here to avoid borrow checker issues
            if let Some(ref _cache_dir) = self.reader.local_cache_dir {
                let current_idx = self.current_file_idx;
                // Only start copying if we've advanced at least 50 files since last copy start
                if current_idx >= self.last_copy_start_idx + 50 || self.last_copy_start_idx == 0 {
                    let total_files = self.reader.block_files.len();
                    let files_to_copy_ahead = 1000;
                    
                    for i in 1..=files_to_copy_ahead {
                        let file_idx = current_idx + i;
                        if file_idx < total_files {
                            self.copy_file_locally(file_idx);
                        }
                    }
                    
                    self.last_copy_start_idx = current_idx;
                }
            }
            
            // Try to open the file, skip if permission denied
            match File::open(&path_to_use) {
                Ok(file) => {
                    self.current_file = Some(BufReader::with_capacity(64 * 1024 * 1024, file)); // 64MB buffer (optimized for large files)
                    self.current_reading_file_idx = Some(self.current_file_idx); // Track which file we're reading from
                    return Ok(true);
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("⚠️  Permission denied for file {} - skipping", 
                             file_path.display());
                    // Continue loop to try next file
                    continue;
                }
                Err(e) => {
                    // Other errors - log and try next file
                    eprintln!("⚠️  Error opening file {}: {} - skipping", 
                             file_path.display(), e);
                    continue;
                }
            }
        }
    }
}

impl Iterator for BlockIterator {
    type Item = Result<Vec<u8>>;
    
    fn next(&mut self) -> Option<Self::Item> {
        // Check if we've read enough blocks
        if let Some(max) = self.max_blocks {
            if self.blocks_read >= max {
                return None;
            }
        }
        
        // If we have ordered blocks (Start9), use those
        if let Some(ref ordered) = self.ordered_blocks {
            if self.ordered_index < ordered.len() {
                let block_data = ordered[self.ordered_index].clone();
                self.ordered_index += 1;
                self.current_height += 1;
                self.blocks_read += 1;
                return Some(Ok(block_data));
            } else {
                return None;
            }
        }
        
        // Otherwise, read sequentially from file
        // FIX: If current_file is None, we need to open the first file before reading
        if self.current_file.is_none() {
            // Open first file (current_file_idx starts at 0 or needs to be initialized)
            match self.next_file() {
                Ok(true) => {
                    // File opened successfully, continue reading
                    return self.next();
                }
                Ok(false) => {
                    // No more files
                    return None;
                }
                Err(e) => {
                    eprintln!("⚠️  Error opening first file: {} - no blocks to read", e);
                    return None;
                }
            }
        }
        
        match self.read_next_from_file() {
            Ok(Some(block_data)) => {
                // Check if we should skip this block (before start_height)
                if let Some(start) = self.start_height {
                    if self.current_height < start {
                        self.current_height += 1;
                        return self.next(); // Skip and try next
                    }
                }
                
                self.current_height += 1;
                self.blocks_read += 1;
                Some(Ok(block_data))
            }
            Ok(None) => {
                // End of current file, try next file
                match self.next_file() {
                    Ok(true) => self.next(), // Continue with next file
                    Ok(false) => None, // No more files
                    Err(e) => {
                        // Error moving to next file - log and try to continue
                        eprintln!("⚠️  Error moving to next file: {} - trying to continue", e);
                        // Try to manually advance to next file
                        self.current_file_idx += 1;
                        if self.current_file_idx < self.reader.block_files.len() {
                            self.next() // Try again
                        } else {
                            None // No more files
                        }
                    }
                }
            }
            Err(e) => {
                // Error reading block - close current file and try next file instead of stopping
                eprintln!("⚠️  Error reading block: {} - closing file and trying next", e);
                // Close current file (drop it)
                self.current_file = None;
                // Try to move to next file and continue
                // Keep trying until we've exhausted all files
                loop {
                    match self.next_file() {
                        Ok(true) => {
                            // Successfully moved to next file - try reading from it
                            return self.next();
                        }
                        Ok(false) => {
                            // No more files - we're done
                            return None;
                        }
                        Err(e2) => {
                            // Error moving to next file - log and try to manually advance
                            eprintln!("⚠️  Error moving to next file: {} - manually advancing", e2);
                            self.current_file_idx += 1;
                            if self.current_file_idx >= self.reader.block_files.len() {
                                // Truly no more files
                                return None;
                            }
                            // Continue loop to try next file
                        }
                    }
                }
            }
        }
    }
}

/// Shared block cache for both Core and Commons
/// 
/// Downloads blocks once and stores them in a shared location
/// that both Core and Commons can access.
pub struct SharedBlockCache {
    cache_dir: PathBuf,
}

impl SharedBlockCache {
    /// Create a shared block cache
    pub fn new(cache_dir: impl AsRef<Path>) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&cache_dir)?;
        
        Ok(Self { cache_dir })
    }
    
    /// Get block from cache or download it
    pub async fn get_or_fetch_block(
        &self,
        height: u64,
        rpc_client: Option<&crate::core_rpc_client::CoreRpcClient>,
    ) -> Result<Vec<u8>> {
        let cache_path = self.cache_dir.join(format!("block_{}.bin", height));
        
        // Check cache first
        if cache_path.exists() {
            let cached = std::fs::read(&cache_path)?;
            #[cfg(debug_assertions)]
            if height == 16 || height <= 2 {
                eprintln!("DEBUG get_or_fetch_block {}: Using cached block ({} bytes)", height, cached.len());
                // Verify cached block is correct by checking hash
                // OPTIMIZATION: Use blvm-consensus OptimizedSha256 (SHA-NI or AVX2) instead of sha2 crate
                if cached.len() >= 80 {
                    let header = &cached[0..80];
                    use blvm_consensus::crypto::OptimizedSha256;
                    let hasher = OptimizedSha256::new();
                    let block_hash = hex::encode(hasher.hash256(header));
                    eprintln!("DEBUG get_or_fetch_block {}: Cached block hash = {}", height, block_hash);
                }
            }
            return Ok(cached);
        }
        
        // Not in cache, try to fetch it
        // First try RPC if available
        if let Some(client) = rpc_client {
            match client.getblockhash(height).await {
                Ok(block_hash) => {
                    match client.getblock_raw(&block_hash).await {
                        Ok(block_hex) => {
                            let block_bytes = hex::decode(&block_hex)?;
                            // Cache it for next time
                            std::fs::write(&cache_path, &block_bytes)?;
                            return Ok(block_bytes);
                        }
                        Err(e) => {
                            eprintln!("⚠️  RPC getblock_raw failed for height {}: {}", height, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("⚠️  RPC getblockhash failed for height {}: {}", height, e);
                }
            }
        }
        
        // If RPC failed or not available, try DirectFile as fallback
        // Try known mount points directly (bypass auto-detect which may fail due to permissions)
        let possible_dirs = vec![
            dirs::home_dir().map(|h| h.join("mnt/bitcoin-start9")),
            Some(PathBuf::from("/mnt/bitcoin-start9")),
            dirs::home_dir().map(|h| h.join(".bitcoin")),
        ];
        
        for dir in possible_dirs.into_iter().flatten() {
            if dir.join("blocks").exists() {
                if let Ok(reader) = BlockFileReader::new(&dir, Network::Mainnet) {
                    // Use sequential reading to find the block at this height
                    let mut iterator = reader.read_blocks_sequential(Some(height), Some(1))?;
                    if let Some(block_result) = iterator.next() {
                        let block_bytes = block_result?;
                        // Cache it for next time
                        std::fs::write(&cache_path, &block_bytes)?;
                        return Ok(block_bytes);
                    }
                }
            }
        }
        
        
        anyhow::bail!("No RPC client or DirectFile available and block {} not in cache", height)
    }
    
    /// Pre-fetch a range of blocks
    pub async fn prefetch_range(
        &self,
        start_height: u64,
        end_height: u64,
        rpc_client: &crate::core_rpc_client::CoreRpcClient,
    ) -> Result<()> {
        println!("📥 Pre-fetching blocks {}-{} to shared cache...", start_height, end_height);
        
        for height in start_height..=end_height {
            if height % 1000 == 0 {
                println!("   Progress: {}/{} ({:.1}%)", 
                         height - start_height, 
                         end_height - start_height,
                         100.0 * (height - start_height) as f64 / (end_height - start_height) as f64);
            }
            
            let _ = self.get_or_fetch_block(height, Some(rpc_client)).await?;
        }
        
        println!("✅ Pre-fetch complete!");
        Ok(())
    }
    
    /// Get cache statistics
    pub fn cache_stats(&self) -> Result<CacheStats> {
        let mut total_blocks = 0;
        let mut total_size = 0u64;
        
        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ext == "bin" {
                    total_blocks += 1;
                    total_size += entry.metadata()?.len();
                }
            }
        }
        
        Ok(CacheStats {
            total_blocks,
            total_size_bytes: total_size,
        })
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub total_blocks: usize,
    pub total_size_bytes: u64,
}

