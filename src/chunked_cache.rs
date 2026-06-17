//! Chunked and compressed cache support
//!
//! Handles reading from chunked, compressed cache files created by split_and_compress_cache.sh
//! Format: Multiple files like chunk_0.bin.zst, chunk_1.bin.zst, etc.

use crate::chunk_index::{
    BlockIndex, BlockIndexEntry, build_block_index, load_block_index, save_block_index,
};
use crate::node_rpc_client::{NodeRpcClient, RpcConfig};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

/// `KERNEL_DIFF_RPC_CHUNK_SKIP_MB` (MiB): if a zstd chunk seek would skip at least this many
/// **decompressed** bytes, fetch the block via Bitcoin RPC (`getblockhash` + `getblock` verbosity 0)
/// instead and switch to RPC for **all** remaining blocks in this iterator.  Requires
/// `BITCOIN_RPC_*` (see [`RpcConfig::from_env`]).  `0` or unset = disabled (chunk-only).
///
/// In RPC mode, the next height is **prefetched** on a Tokio worker while the caller validates the
/// current block (overlaps I/O with CPU). HTTP keep-alive + idle pool are enabled on [`NodeRpcClient`].
fn rpc_chunk_skip_bytes_from_env() -> u64 {
    if let Ok(s) = std::env::var("KERNEL_DIFF_RPC_CHUNK_SKIP_BYTES") {
        if let Ok(n) = s.parse::<u64>() {
            return n;
        }
    }
    std::env::var("KERNEL_DIFF_RPC_CHUNK_SKIP_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
        .saturating_mul(1024 * 1024)
}

fn global_tokio_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for KERNEL_DIFF RPC chunk fallback")
    })
}

/// Chunk metadata
#[derive(Debug, Clone)]
pub struct ChunkMetadata {
    pub total_blocks: u64,
    pub num_chunks: usize,
    pub blocks_per_chunk: u64,
    pub compression: String,
}

/// Load chunk metadata from chunks.meta file
pub fn load_chunk_metadata(chunks_dir: &Path) -> Result<Option<ChunkMetadata>> {
    let meta_file = chunks_dir.join("chunks.meta");
    if !meta_file.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&meta_file)?;
    let mut total_blocks = None;
    let mut num_chunks = None;
    let mut blocks_per_chunk = None;
    let mut compression = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "total_blocks" => total_blocks = value.trim().parse().ok(),
                "num_chunks" => num_chunks = value.trim().parse().ok(),
                "blocks_per_chunk" => blocks_per_chunk = value.trim().parse().ok(),
                "compression" => compression = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }

    if let (Some(total), Some(num), Some(per_chunk), Some(comp)) =
        (total_blocks, num_chunks, blocks_per_chunk, compression)
    {
        Ok(Some(ChunkMetadata {
            total_blocks: total,
            num_chunks: num,
            blocks_per_chunk: per_chunk,
            compression: comp,
        }))
    } else {
        Ok(None)
    }
}

/// Decompress a zstd-compressed chunk file
///
/// OPTIMIZATION: Returns a streaming reader instead of loading entire chunk into memory
/// This prevents OOM for large chunks (50-60GB compressed = 200GB+ uncompressed)
pub fn decompress_chunk_streaming(chunk_path: &Path) -> Result<std::process::Child> {
    decompress_chunk_streaming_mt(chunk_path, 1)
}

/// Decompress with multi-threading support
///
/// Uses zstd's -T flag for parallel decompression (zstd 1.5+)
/// threads=0 means use all available cores
pub fn decompress_chunk_streaming_mt(
    chunk_path: &Path,
    threads: usize,
) -> Result<std::process::Child> {
    use std::process::{Command, Stdio};

    // OPTIMIZATION: Use streaming decompression with multi-threading
    let child = Command::new("zstd")
        .arg("-d")
        .arg("--stdout")
        .arg(format!("-T{}", threads)) // Multi-threaded decompression
        .arg(chunk_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "Failed to start zstd decompression: {}",
                chunk_path.display()
            )
        })?;

    Ok(child)
}

/// Decompress a zstd-compressed chunk file (legacy - loads entire chunk)
///
/// WARNING: This loads the entire chunk into memory. For large chunks (50-60GB compressed),
/// this can require 200GB+ RAM. Use decompress_chunk_streaming() instead.
#[allow(dead_code)]
pub fn decompress_chunk(chunk_path: &Path) -> Result<Vec<u8>> {
    use std::process::Command;

    // Check if zstd is available
    let output = Command::new("zstd")
        .arg("--version")
        .output()
        .context("zstd not found - install with: sudo pacman -S zstd")?;

    if !output.status.success() {
        anyhow::bail!("zstd command failed");
    }

    // Decompress chunk
    let output = Command::new("zstd")
        .arg("-d")
        .arg("--stdout")
        .arg(chunk_path)
        .output()
        .with_context(|| format!("Failed to decompress chunk: {}", chunk_path.display()))?;

    if !output.status.success() {
        anyhow::bail!(
            "zstd decompression failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(output.stdout)
}

/// Load blocks from a single chunk
pub fn load_chunk_blocks(chunk_data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut blocks = Vec::new();
    let mut offset = 0usize;

    while offset + 4 <= chunk_data.len() {
        // Read block length (u32)
        let block_len = u32::from_le_bytes([
            chunk_data[offset],
            chunk_data[offset + 1],
            chunk_data[offset + 2],
            chunk_data[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + block_len > chunk_data.len() {
            anyhow::bail!("Block extends beyond chunk data");
        }

        blocks.push(chunk_data[offset..offset + block_len].to_vec());
        offset += block_len;
    }

    Ok(blocks)
}

/// Advise the kernel to drop page-cache pages for `path` using `POSIX_FADV_DONTNEED`.
///
/// Called **before** spawning the zstd subprocess that will read the chunk file, and again
/// periodically during long seeks.  Because zstd owns its own file descriptor, we open the file
/// separately just to make the syscall — `posix_fadvise` applies to the *page cache* (shared by
/// all descriptors for the same inode), so our fd doesn't need to be the same one zstd uses.
/// On non-Linux platforms this is a no-op.
#[cfg(target_os = "linux")]
fn fadvise_dontneed(path: &Path) {
    use std::os::unix::io::IntoRawFd;
    if let Ok(f) = std::fs::File::open(path) {
        let fd = f.into_raw_fd();
        unsafe {
            // POSIX_FADV_DONTNEED = 4.  len=0 means "to end of file".
            libc::posix_fadvise(fd, 0, 0, 4 /* POSIX_FADV_DONTNEED */);
            libc::close(fd);
        }
    }
}
#[cfg(not(target_os = "linux"))]
fn fadvise_dontneed(_path: &Path) {}

/// Spawn `zstd -d --stdout` reading `chunk_file`, with clear errors when the **`zstd` binary** is
/// missing (often reported as bare `No such file or directory` by the OS).
fn spawn_zstd_decompress_stdout(
    chunk_file: &Path,
    multi_thread_decode: bool,
) -> Result<std::process::Child> {
    use std::io::ErrorKind;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("zstd");
    cmd.arg("-d").arg("--stdout").arg("-q");
    if multi_thread_decode {
        let zstd_threads = std::cmp::min(6, num_cpus::get().saturating_sub(2));
        cmd.arg(format!("-T{}", zstd_threads));
    }
    cmd.arg(chunk_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    cmd.spawn().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            anyhow::anyhow!(
                "cannot run the `zstd` decompressor: no executable named `zstd` on PATH (os error: {}). \
                 Install the `zstd` package and ensure `zstd` works in your shell. \
                 Chunk archive: {}",
                e,
                chunk_file.display()
            )
        } else {
            anyhow::anyhow!(
                "failed to spawn `zstd -d` on {}: {}",
                chunk_file.display(),
                e
            )
        }
    })
}

/// Create a streaming iterator over blocks from chunked cache
/// This yields blocks one at a time without loading all into memory
/// Uses block index to ensure correct ordering by height
pub struct ChunkedBlockIterator {
    chunks_dir: PathBuf,
    metadata: ChunkMetadata,
    index: Arc<BlockIndex>, // Block index for correct ordering (Arc for sharing)
    /// When true, each yielded block's header hash is checked against `chunks.index` (two SHA256s per block).
    /// Set false when the index was already validated (e.g. `validate_utxo_chunk_cache_index`) for higher throughput.
    verify_block_hash_against_index: bool,
    start_height: u64,
    end_height: u64,
    current_height: u64,
    current_chunk_reader: Option<std::io::BufReader<std::process::ChildStdout>>,
    current_zstd_proc: Option<std::process::Child>,
    current_chunk_number: Option<usize>,
    current_offset: u64,
    /// Path of the currently open chunk file; used by the seek loop to call FADV_DONTNEED
    /// periodically so the 60 GB seek does not accumulate 5+ GiB of OS page cache.
    current_chunk_file: Option<PathBuf>,
    /// Tracks decompressed bytes read since the last fadvise_dontneed call during comparison
    /// (not the seek phase).  We call DONTNEED roughly every 256 MiB of reads to keep the
    /// chunk's compressed page-cache from accumulating during the multi-hour comparison loop.
    fadvise_decompressed_since_last: u64,
    /// If non-zero and [`Self::rpc_only_mode`] is false, a chunk seek of at least this many
    /// **decompressed** bytes triggers RPC fallback (see module docs above).
    rpc_chunk_skip_bytes: u64,
    /// After a large seek was avoided via RPC, we keep using RPC — the zstd stream cannot
    /// be advanced without decompressing, so chunk reads are abandoned for this run.
    rpc_only_mode: bool,
    /// Lazily created when RPC fallback first activates.
    rpc_client: Option<Box<NodeRpcClient>>,
    /// Background fetch for `current_height + 1` while the caller processes the current block.
    rpc_prefetch: Option<tokio::task::JoinHandle<anyhow::Result<Vec<u8>>>>,
    rpc_prefetch_height: Option<u64>,
}

impl ChunkedBlockIterator {
    /// Height the iterator will return on the next successful [`Self::next_block`] call.
    #[inline]
    pub fn current_height(&self) -> u64 {
        self.current_height
    }

    /// Raise [`Self::end_height`] so reading can continue after a lookahead window (e.g. header
    /// prefetch through `H` with `--start == H+1`). Stream position must already be at `H+1`.
    /// `compare_start` must match [`Self::current_height`]. `max_blocks` matches [`Self::new`]'s
    /// third argument (exclusive end is `compare_start + max` capped by chain tip).
    pub fn extend_end_for_compare(
        &mut self,
        compare_start: u64,
        max_blocks: Option<usize>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.current_height == compare_start,
            "extend_end_for_compare: iterator at height {} but --start is {}",
            self.current_height,
            compare_start
        );
        self.end_height = if let Some(max) = max_blocks {
            (compare_start + max as u64).min(self.metadata.total_blocks)
        } else {
            self.metadata.total_blocks
        };
        Ok(())
    }

    /// Create a new iterator with a pre-loaded block index (faster for repeated use)
    pub fn new_with_index(
        chunks_dir: &Path,
        index: Arc<BlockIndex>,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
        verify_block_hash_against_index: bool,
    ) -> Result<Option<Self>> {
        let metadata = match load_chunk_metadata(chunks_dir)? {
            Some(m) => m,
            None => return Ok(None),
        };

        let start_height_val = start_height.unwrap_or(0);
        let end_height_val = if let Some(max) = max_blocks {
            (start_height_val + max as u64).min(metadata.total_blocks)
        } else {
            metadata.total_blocks
        };

        // Verify index has all required blocks
        for h in start_height_val..end_height_val.min(100) {
            if !index.contains_key(&h) {
                anyhow::bail!("Block index missing entry for height {}", h);
            }
        }

        let rpc_chunk_skip_bytes = rpc_chunk_skip_bytes_from_env();
        Ok(Some(Self {
            chunks_dir: chunks_dir.to_path_buf(),
            metadata,
            index,
            verify_block_hash_against_index,
            start_height: start_height_val,
            end_height: end_height_val,
            current_height: start_height_val,
            current_chunk_reader: None,
            current_zstd_proc: None,
            current_chunk_number: None,
            current_offset: 0,
            current_chunk_file: None,
            fadvise_decompressed_since_last: 0,
            rpc_chunk_skip_bytes,
            rpc_only_mode: false,
            rpc_client: None,
            rpc_prefetch: None,
            rpc_prefetch_height: None,
        }))
    }

    /// Same as [`Self::new`] but skips per-block header hash checks against the index (faster; trust `chunks.index`).
    pub fn new_trust_chunk_index(
        chunks_dir: &Path,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
    ) -> Result<Option<Self>> {
        Self::new_with_hash_policy(chunks_dir, start_height, max_blocks, false)
    }

    /// After validating the chunk index, use [`Self::new_trust_chunk_index`] to avoid two SHA256s per block.
    pub fn new(
        chunks_dir: &Path,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
    ) -> Result<Option<Self>> {
        Self::new_with_hash_policy(chunks_dir, start_height, max_blocks, true)
    }

    fn new_with_hash_policy(
        chunks_dir: &Path,
        start_height: Option<u64>,
        max_blocks: Option<usize>,
        verify_block_hash_against_index: bool,
    ) -> Result<Option<Self>> {
        // Load or build block index for correct ordering
        let index = match load_block_index(chunks_dir)? {
            Some(idx) => {
                println!("   ✅ Loaded block index ({} entries)", idx.len());
                idx
            }
            None => {
                println!("   🔨 Block index not found, building...");
                println!("   ⚠️  This may take a while (reading all blocks from chunks)...");

                // Try building index via chaining
                let idx = match build_block_index(chunks_dir) {
                    Ok((idx, _)) if idx.len() > 1 => {
                        // Chaining succeeded
                        idx
                    }
                    Ok((idx, _)) => {
                        // Chaining returned partial index (likely missing block 1)
                        println!(
                            "   ⚠️  Chaining returned partial index ({} entries) - likely missing blocks",
                            idx.len()
                        );
                        println!(
                            "   💡 Missing blocks will be fetched from RPC during async index build"
                        );
                        idx
                    }
                    Err(e) => {
                        // Chaining failed
                        eprintln!("   ⚠️  Chaining failed: {}", e);
                        eprintln!("   ⚠️  Returning empty index - will use RPC-based indexing");
                        BlockIndex::new()
                    }
                };

                if idx.len() > 1 {
                    save_block_index(chunks_dir, &idx)?;
                    println!("   ✅ Built and saved block index ({} entries)", idx.len());
                } else {
                    eprintln!("   ⚠️  Index build incomplete (only {} entries)", idx.len());
                    eprintln!("   💡 This is expected if block 1 is missing from chunks");
                    eprintln!("   💡 Index will be built via RPC in async context");
                    // Don't save incomplete index - will be rebuilt with RPC
                }
                idx
            }
        };
        let metadata = match load_chunk_metadata(chunks_dir)? {
            Some(m) => m,
            None => return Ok(None),
        };

        let start_height_val = start_height.unwrap_or(0);
        let end_height_val = if let Some(max) = max_blocks {
            (start_height_val + max as u64).min(metadata.total_blocks)
        } else {
            metadata.total_blocks
        };

        // Verify index has all required blocks
        for h in start_height_val..end_height_val.min(100) {
            if !index.contains_key(&h) {
                anyhow::bail!("Block index missing entry for height {}", h);
            }
        }

        let rpc_chunk_skip_bytes = rpc_chunk_skip_bytes_from_env();
        Ok(Some(Self {
            chunks_dir: chunks_dir.to_path_buf(),
            metadata,
            index: Arc::new(index),
            verify_block_hash_against_index,
            start_height: start_height_val,
            end_height: end_height_val,
            current_height: start_height_val,
            current_chunk_reader: None,
            current_zstd_proc: None,
            current_chunk_number: None,
            current_offset: 0,
            current_chunk_file: None,
            fadvise_decompressed_since_last: 0,
            rpc_chunk_skip_bytes,
            rpc_only_mode: false,
            rpc_client: None,
            rpc_prefetch: None,
            rpc_prefetch_height: None,
        }))
    }

    fn ensure_rpc_client(&mut self) -> Result<&NodeRpcClient> {
        if self.rpc_client.is_none() {
            self.rpc_client = Some(Box::new(NodeRpcClient::new(RpcConfig::from_env())));
        }
        Ok(self.rpc_client.as_deref().unwrap())
    }

    fn enter_rpc_only_mode(&mut self) {
        if self.rpc_only_mode {
            return;
        }
        // Do not cancel rpc_prefetch here: load_block_from_index calls fetch_block_via_rpc (which
        // schedules the next height) before this method.
        if let Some(mut proc) = self.current_zstd_proc.take() {
            let _ = proc.kill();
            let _ = proc.wait();
        }
        self.current_chunk_reader = None;
        self.current_chunk_number = None;
        self.current_offset = 0;
        if let Some(ref cf) = self.current_chunk_file.take() {
            fadvise_dontneed(cf);
        }
        self.rpc_only_mode = true;
        eprintln!(
            "   📡 KERNEL_DIFF_RPC_CHUNK_SKIP: large chunk seek avoided — using getblock (RPC) for remaining blocks"
        );
        eprintln!(
            "      Configure BITCOIN_RPC_HOST / BITCOIN_RPC_USER / BITCOIN_RPC_PASSWORD; unset KERNEL_DIFF_RPC_CHUNK_SKIP_MB to disable"
        );
    }

    fn rpc_cancel_prefetch(&mut self) {
        if let Some(handle) = self.rpc_prefetch.take() {
            handle.abort();
        }
        self.rpc_prefetch_height = None;
    }

    fn rpc_schedule_prefetch(&mut self, next_height: u64) {
        self.rpc_cancel_prefetch();
        if next_height >= self.end_height {
            return;
        }
        let Some(client) = self.rpc_client.as_ref().map(|b| (**b).clone()) else {
            return;
        };
        let rt = global_tokio_runtime();
        let handle = rt.spawn(async move { client.getblock_bytes_at_height(next_height).await });
        self.rpc_prefetch_height = Some(next_height);
        self.rpc_prefetch = Some(handle);
    }

    fn fetch_block_via_rpc(&mut self, height: u64) -> Result<Vec<u8>> {
        let rt = global_tokio_runtime();
        let client = self.ensure_rpc_client()?.clone();

        if self.rpc_prefetch_height == Some(height) {
            if let Some(handle) = self.rpc_prefetch.take() {
                self.rpc_prefetch_height = None;
                match rt.block_on(async { handle.await }) {
                    Ok(Ok(block)) => {
                        self.rpc_schedule_prefetch(height + 1);
                        return Ok(block);
                    }
                    Ok(Err(e)) => {
                        eprintln!(
                            "   ⚠️  RPC prefetch failed at height {height}: {e}; retrying sync fetch"
                        );
                    }
                    Err(join_err) => {
                        eprintln!(
                            "   ⚠️  RPC prefetch join at height {height}: {join_err}; retrying sync fetch"
                        );
                    }
                }
            } else {
                self.rpc_prefetch_height = None;
            }
        } else {
            self.rpc_cancel_prefetch();
        }

        let block = rt
            .block_on(async { client.getblock_bytes_at_height(height).await })
            .with_context(|| format!("RPC block fetch failed at height {height}"))?;
        self.rpc_schedule_prefetch(height + 1);
        Ok(block)
    }

    fn load_block_from_index(&mut self, height: u64) -> Result<Option<Vec<u8>>> {
        if self.rpc_only_mode {
            return Ok(Some(self.fetch_block_via_rpc(height)?));
        }

        let entry = match self.index.get(&height) {
            Some(e) => e,
            None => return Ok(None),
        };
        let _height = height;

        let need_new_chunk = self.current_chunk_number != Some(entry.chunk_number);

        // Decompressed bytes we would skip before the block payload (same as the seek loop below).
        // If this exceeds KERNEL_DIFF_RPC_CHUNK_SKIP_* and RPC is configured, avoid opening the
        // zstd stream / long seek and use getblock for this and all following heights.
        let skip_bytes = entry.offset_in_chunk.saturating_sub(if need_new_chunk {
            0
        } else {
            self.current_offset
        });
        if self.rpc_chunk_skip_bytes > 0 && skip_bytes >= self.rpc_chunk_skip_bytes {
            // Fetch first so we stay on chunk path if RPC is misconfigured.
            let block = self.fetch_block_via_rpc(height)?;
            self.enter_rpc_only_mode();
            return Ok(Some(block));
        }

        if need_new_chunk {
            if let Some(mut proc) = self.current_zstd_proc.take() {
                let _ = proc.kill();
                let _ = proc.wait();
            }
            self.current_chunk_reader = None;
            // Drop residual page-cache pages for the chunk we just finished with.
            if let Some(ref old_chunk_file) = self.current_chunk_file.take() {
                fadvise_dontneed(old_chunk_file);
            }

            if entry.chunk_number == 999 {
                use crate::missing_blocks::get_missing_block;
                let result = get_missing_block(&self.chunks_dir, height);
                return match result? {
                    Some(block_data) => Ok(Some(block_data)),
                    None => {
                        eprintln!(
                            "   ⚠️  Missing block {} not found in chunk_missing — skipping",
                            height
                        );
                        Ok(None)
                    }
                };
            }

            let chunk_file = self
                .chunks_dir
                .join(format!("chunk_{}.bin.zst", entry.chunk_number));
            if !chunk_file.exists() {
                anyhow::bail!(
                    "Chunk {} not found: {}",
                    entry.chunk_number,
                    chunk_file.display()
                );
            }
            eprintln!(
                "   📦 Opening chunk {} for height {}",
                entry.chunk_number, height
            );

            // Drop any existing page-cache pages for the chunk file before the zstd subprocess
            // opens it.  Sequential read of a 60 GB file fills ~5 GiB of OS page cache and drives
            // MemAvailable below the safety floor.  posix_fadvise(DONTNEED) keeps page-cache usage
            // near-zero for data we've already consumed.
            fadvise_dontneed(&chunk_file);

            let mut zstd_proc = spawn_zstd_decompress_stdout(&chunk_file, true)?;
            let stdout = zstd_proc
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
            // 16 MiB read-ahead is ample; the old 128 MiB buffer held unnecessary anonymous pages.
            let reader = std::io::BufReader::with_capacity(16 * 1024 * 1024, stdout);

            self.current_chunk_reader = Some(reader);
            self.current_zstd_proc = Some(zstd_proc);
            self.current_chunk_number = Some(entry.chunk_number);
            self.current_offset = 0;
            // Store chunk file path so the seek loop can call DONTNEED periodically.
            self.current_chunk_file = Some(chunk_file);
        }

        // Seek to block offset (read and discard bytes until we reach offset)
        let reader = self
            .current_chunk_reader
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No chunk reader available"))?;

        if self.current_offset < entry.offset_in_chunk {
            let skip_bytes = entry.offset_in_chunk - self.current_offset;
            // Use 64MB skip buffer - 1MB was too slow for large chunks (50k+ read() calls for 50GB seek)
            const SKIP_BUF_SIZE: u64 = 64 * 1024 * 1024;
            let mut skip_buf = vec![0u8; (skip_bytes.min(SKIP_BUF_SIZE)) as usize];
            let mut remaining = skip_bytes;
            let mut skipped_so_far = 0u64;
            let progress_interval = 1024 * 1024 * 1024; // Log every 1GB
            let total_gb = skip_bytes as f64 / 1e9;
            if total_gb > 0.1 {
                eprintln!(
                    "   ⏳ Seeking to block {} in chunk {} ({:.1}GB to skip)...",
                    height, entry.chunk_number, total_gb
                );
            }

            while remaining > 0 {
                let to_read = remaining.min(skip_buf.len() as u64) as usize;
                use std::io::Read;
                let bytes_read = reader.read(&mut skip_buf[..to_read]).with_context(|| {
                    format!(
                        "Failed to read from chunk stream at offset {}",
                        self.current_offset
                    )
                })?;
                if bytes_read == 0 {
                    anyhow::bail!(
                        "Unexpected EOF while seeking to block offset (current={}, needed={})",
                        self.current_offset,
                        entry.offset_in_chunk
                    );
                }
                remaining -= bytes_read as u64;
                skipped_so_far += bytes_read as u64;
                let prev_gb = (skipped_so_far - bytes_read as u64) / progress_interval;
                let curr_gb = skipped_so_far / progress_interval;
                if curr_gb > prev_gb && total_gb > 0.1 {
                    eprintln!(
                        "   ⏳ Seeking in chunk {}: {:.1}GB / {:.1}GB...",
                        entry.chunk_number,
                        skipped_so_far as f64 / 1e9,
                        total_gb
                    );
                    // Every 8 GB of decompressed data skipped, tell the kernel to drop the
                    // compressed chunk's page-cache pages we've already consumed.  The
                    // compression ratio is ~4–6×, so 8 GB decompressed ≈ 1.5–2 GB of file
                    // pages freed each time.  DONTNEED on the whole file is coarse but safe.
                    if curr_gb % 8 == 0 {
                        if let Some(ref cf) = self.current_chunk_file {
                            fadvise_dontneed(cf);
                        }
                    }
                }
            }
            self.current_offset = entry.offset_in_chunk;
            // Seek done: drop all page-cache for the chunk — we're now positioned and the pages
            // for the skipped region are no longer needed.
            if let Some(ref cf) = self.current_chunk_file {
                fadvise_dontneed(cf);
            }
        } else if self.current_offset > entry.offset_in_chunk {
            // Can't seek backwards in a stream - need to restart chunk
            // This shouldn't happen if we're reading in order, but handle it
            anyhow::bail!(
                "Cannot seek backwards in chunk stream (current={}, needed={})",
                self.current_offset,
                entry.offset_in_chunk
            );
        }

        // Read block length (4 bytes)
        let mut len_buf = [0u8; 4];
        use std::io::Read;
        reader.read_exact(&mut len_buf).with_context(|| {
            format!(
                "Failed to read block length at height {} (offset {})",
                height, self.current_offset
            )
        })?;

        self.current_offset += 4;

        let block_len = u32::from_le_bytes(len_buf) as usize;
        if block_len > 10 * 1024 * 1024 || block_len < 88 {
            anyhow::bail!(
                "Invalid block size: {} bytes (height {}, offset {})",
                block_len,
                height,
                self.current_offset
            );
        }

        // Read block data
        let mut block_data = vec![0u8; block_len];
        let data_read_start = std::time::Instant::now();
        reader.read_exact(&mut block_data).with_context(|| {
            format!(
                "Failed to read block data at height {} (offset {}, len {})",
                height, self.current_offset, block_len
            )
        })?;
        let data_read_duration = data_read_start.elapsed();
        if data_read_duration.as_secs() > 1 {
            eprintln!(
                "   ⚠️  Slow block read: height {} took {:.2}s ({} bytes)",
                height,
                data_read_duration.as_secs_f64(),
                block_len
            );
        }

        self.current_offset += block_len as u64;

        // During the comparison loop, sequential block reads fill the OS page cache with the
        // compressed chunk file data (~4 MB/s of new page cache).  Call fadvise_dontneed every
        // 256 MiB of decompressed reads to prevent this from eroding MemAvailable.
        self.fadvise_decompressed_since_last += (block_len + 4) as u64;
        if self.fadvise_decompressed_since_last >= 256 * 1024 * 1024 {
            self.fadvise_decompressed_since_last = 0;
            if let Some(ref cf) = self.current_chunk_file {
                fadvise_dontneed(cf);
            }
        }

        Ok(Some(block_data))
    }

    pub fn next_block(&mut self) -> Result<Option<Vec<u8>>> {
        // CRITICAL FIX: Skip missing blocks instead of stopping
        loop {
            if self.current_height >= self.end_height {
                return Ok(None);
            }

            // OPTIMIZATION: For sequential reading, read directly from stream without seeking
            // This eliminates expensive skip operations when reading blocks in order
            // Only works if we're already in the right chunk and at the right position
            if let Some(ref mut reader) = self.current_chunk_reader {
                // Check if we're at the expected position for sequential read
                // If current_offset matches expected offset from index, we can read sequentially
                if let Some(entry) = self.index.get(&self.current_height) {
                    if self.current_chunk_number == Some(entry.chunk_number)
                        && self.current_offset == entry.offset_in_chunk
                    {
                        // We're at the right position - read sequentially (fast path)
                        use std::io::Read;
                        let mut len_buf = [0u8; 4];
                        match reader.read_exact(&mut len_buf) {
                            Ok(_) => {
                                let block_len = u32::from_le_bytes(len_buf) as usize;
                                if block_len <= 10 * 1024 * 1024 && block_len >= 88 {
                                    let mut block_data = vec![0u8; block_len];
                                    match reader.read_exact(&mut block_data) {
                                        Ok(_) => {
                                            // Sequential read succeeded - update offset and return
                                            self.current_offset += 4 + block_len as u64;
                                            let height = self.current_height;
                                            self.current_height += 1;
                                            return Ok(Some(block_data));
                                        }
                                        Err(_) => {
                                            // Read failed - fall through to index-based loading
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                // Sequential read failed (maybe end of chunk) - use index
                            }
                        }
                    }
                }
            }

            // Fallback: Use index to load block (for non-sequential access or chunk boundaries)
            match self.load_block_from_index(self.current_height) {
                Ok(Some(block)) => {
                    if self.verify_block_hash_against_index && block.len() >= 80 {
                        use sha2::{Digest, Sha256};
                        let header = &block[0..80];
                        let first_hash = Sha256::digest(header);
                        let second_hash = Sha256::digest(&first_hash);
                        let mut block_hash = [0u8; 32];
                        block_hash.copy_from_slice(&second_hash);
                        block_hash.reverse();
                        if let Some(entry) = self.index.get(&self.current_height) {
                            if block_hash != entry.block_hash {
                                eprintln!(
                                    "   ⚠️  Block hash mismatch at height {}! expected={} got={}",
                                    self.current_height,
                                    hex::encode(entry.block_hash),
                                    hex::encode(block_hash)
                                );
                            }
                        }
                    }
                    self.current_height += 1;
                    return Ok(Some(block));
                }
                Ok(None) => {
                    eprintln!(
                        "   ⚠️  Block {} missing from index — skipping",
                        self.current_height
                    );
                    self.current_height += 1;
                    continue;
                }
                Err(e) => {
                    let error_height = self.current_height;
                    eprintln!(
                        "   ❌ Chunked cache: failed loading block at height {}.",
                        error_height
                    );
                    eprintln!("       {:#}", e);
                    return Err(e.context(format!(
                        "chunked block read failed at height {} (common cause: missing `zstd` on PATH or missing chunk file)",
                        error_height
                    )));
                }
            };
        }
    }
}

impl Drop for ChunkedBlockIterator {
    fn drop(&mut self) {
        if let Some(h) = self.rpc_prefetch.take() {
            h.abort();
        }
    }
}

/// Load blocks from chunked cache (legacy - loads all into memory)
///
/// WARNING: This loads all blocks into memory. For large ranges, use ChunkedBlockIterator instead.
/// This function is kept for backward compatibility but should not be used for >10k blocks.
pub fn load_chunked_cache(
    chunks_dir: &Path,
    start_height: Option<u64>,
    max_blocks: Option<usize>,
) -> Result<Option<Vec<Vec<u8>>>> {
    // Load metadata
    let metadata = match load_chunk_metadata(chunks_dir)? {
        Some(m) => m,
        None => {
            // No chunked cache found
            return Ok(None);
        }
    };

    println!(
        "📂 Loading from chunked cache: {} chunks, {} total blocks",
        metadata.num_chunks, metadata.total_blocks
    );

    // Determine which chunks we need
    let start_idx = start_height.unwrap_or(0) as usize;
    let end_idx = if let Some(max) = max_blocks {
        (start_idx + max).min(metadata.total_blocks as usize)
    } else {
        metadata.total_blocks as usize
    };

    let start_chunk = start_idx / metadata.blocks_per_chunk as usize;
    let end_chunk = (end_idx - 1) / metadata.blocks_per_chunk as usize;

    println!(
        "   Loading chunks {}-{} (blocks {}-{})",
        start_chunk, end_chunk, start_idx, end_idx
    );

    // CRITICAL FIX: For large ranges, warn and suggest using DirectFile instead
    // Loading 125,000 blocks = ~187GB memory (125k × 1.5MB avg)
    let total_blocks_to_load = end_idx - start_idx;
    if total_blocks_to_load > 10_000 {
        eprintln!(
            "⚠️  WARNING: Attempting to load {} blocks into memory (requires ~{}GB RAM)",
            total_blocks_to_load,
            (total_blocks_to_load * 1_500_000) / 1_000_000_000
        );
        eprintln!("   💡 For large ranges, use DirectFile source instead of chunked cache");
        eprintln!("   💡 Chunked cache is optimized for small ranges (<10k blocks)");
        eprintln!("   💡 Consider processing in smaller batches or using DirectFile");

        // For very large ranges, return None to force DirectFile usage
        if total_blocks_to_load > 50_000 {
            eprintln!(
                "   ❌ Refusing to load {} blocks - would require ~{}GB RAM",
                total_blocks_to_load,
                (total_blocks_to_load * 1_500_000) / 1_000_000_000
            );
            return Ok(None); // Force fallback to DirectFile
        }
    }

    // OPTIMIZATION: Stream blocks from chunks instead of loading entire chunks into memory
    // For 50-60GB compressed chunks, this prevents loading 200GB+ into RAM
    let mut all_blocks = Vec::new();
    for chunk_num in start_chunk..=end_chunk.min(metadata.num_chunks - 1) {
        let chunk_file = chunks_dir.join(format!("chunk_{}.bin.zst", chunk_num));

        if !chunk_file.exists() {
            eprintln!(
                "   ⚠️  Chunk {} not found: {}",
                chunk_num,
                chunk_file.display()
            );
            continue;
        }

        println!("   📦 Streaming blocks from chunk {}...", chunk_num);

        // OPTIMIZATION: Stream decompression instead of loading entire chunk
        use std::io::{BufReader, Read};

        let mut zstd_proc = spawn_zstd_decompress_stdout(&chunk_file, false)?;

        let mut reader = BufReader::with_capacity(
            128 * 1024 * 1024, // 128MB buffer
            zstd_proc
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?,
        );

        // Read blocks one at a time (streaming)
        let mut blocks_in_chunk = 0;
        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    let _ = zstd_proc.wait(); // Clean up
                    return Err(e.into());
                }
            }

            let block_len = u32::from_le_bytes(len_buf) as usize;

            // Validate block size
            if block_len > 10 * 1024 * 1024 || block_len < 88 {
                let _ = zstd_proc.wait();
                anyhow::bail!(
                    "Invalid block size in chunk {}: {} bytes",
                    chunk_num,
                    block_len
                );
            }

            // Read block data
            let mut block_data = vec![0u8; block_len];
            reader.read_exact(&mut block_data)?;

            all_blocks.push(block_data);
            blocks_in_chunk += 1;

            // OPTIMIZATION: Reduce progress reporting frequency (less I/O overhead)
            if blocks_in_chunk % 25000 == 0 {
                println!(
                    "     Loaded {}/{} blocks from chunk {}...",
                    blocks_in_chunk, metadata.blocks_per_chunk, chunk_num
                );
            }
        }

        // Wait for zstd to finish
        let status = zstd_proc.wait()?;
        if !status.success() {
            anyhow::bail!("zstd decompression failed for chunk {}", chunk_num);
        }

        println!(
            "   ✅ Loaded {} blocks from chunk {}",
            blocks_in_chunk, chunk_num
        );
    }

    // Filter to requested range
    if start_idx > 0 || end_idx < all_blocks.len() {
        let filtered: Vec<_> = all_blocks
            .into_iter()
            .skip(start_idx)
            .take(end_idx - start_idx)
            .collect();
        Ok(Some(filtered))
    } else {
        Ok(Some(all_blocks))
    }
}

/// Get chunk directory path
///
/// 1. `BLOCK_CACHE_DIR` if set, exists, and looks like a chunk dir (`chunks.meta` or `chunk_*.bin.zst`)
/// 2. Else default cache directory (`~/.cache/blvm-bench/chunks`)
pub fn get_chunks_dir() -> Option<PathBuf> {
    if let Ok(env_dir) = std::env::var("BLOCK_CACHE_DIR") {
        if env_dir.is_empty() {
            return fallback_cache_chunks_dir();
        }
        let path = PathBuf::from(env_dir);
        if path.exists()
            && (path.join("chunks.meta").exists()
                || std::fs::read_dir(&path).ok().is_some_and(|rd| {
                    rd.flatten()
                        .any(|e| e.file_name().to_string_lossy().starts_with("chunk_"))
                }))
        {
            return Some(path);
        }
    }

    fallback_cache_chunks_dir()
}

fn fallback_cache_chunks_dir() -> Option<PathBuf> {
    dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
        .map(|cache| cache.join("blvm-bench").join("chunks"))
}

/// Check if chunked cache exists
pub fn chunked_cache_exists() -> bool {
    if let Some(chunks_dir) = get_chunks_dir() {
        chunks_dir.exists() && chunks_dir.join("chunks.meta").exists()
    } else {
        false
    }
}

/// Shared chunk cache manager - decompresses each chunk once and allows concurrent block reads
///
/// CRITICAL OPTIMIZATION: With only 8 chunks total, we can maintain a cache of chunk readers
/// to avoid re-decompressing the same chunk multiple times when loading blocks in parallel.
pub struct SharedChunkCache {
    chunks_dir: PathBuf,
    index: Arc<BlockIndex>,
    // Cache of chunk readers: chunk_number -> (reader, zstd_process, current_offset)
    // CRITICAL: Limited to prevent OOM - each reader holds a zstd process and large buffer
    chunk_readers: Arc<
        Mutex<
            HashMap<
                usize,
                (
                    std::io::BufReader<std::process::ChildStdout>,
                    std::process::Child,
                    u64,
                ),
            >,
        >,
    >,
    max_chunk_readers: usize,
}

impl SharedChunkCache {
    pub fn new(chunks_dir: &Path, index: Arc<BlockIndex>) -> Self {
        Self {
            chunks_dir: chunks_dir.to_path_buf(),
            index,
            chunk_readers: Arc::new(Mutex::new(HashMap::new())),
            max_chunk_readers: 10, // Max 10 concurrent chunk readers (~1-2GB memory)
        }
    }

    /// Load a block directly by height using shared chunk cache
    pub fn load_block(&self, height: u64) -> Result<Option<Vec<u8>>> {
        let entry = match self.index.get(&height) {
            Some(e) => e,
            None => return Ok(None),
        };

        // Handle missing blocks (chunk 999)
        if entry.chunk_number == 999 {
            use crate::missing_blocks::get_missing_block;
            return get_missing_block(&self.chunks_dir, height);
        }

        // Get or create chunk reader
        let mut readers = self.chunk_readers.lock().unwrap();

        // CRITICAL: Evict oldest chunk readers if we're at the limit
        if readers.len() >= self.max_chunk_readers && !readers.contains_key(&entry.chunk_number) {
            // Evict first (oldest) chunk reader
            if let Some(&chunk_num) = readers.keys().next() {
                if let Some((_, mut proc, _)) = readers.remove(&chunk_num) {
                    let _ = proc.kill();
                    let _ = proc.wait();
                }
            }
        }

        // Check if we need to create new chunk reader
        if !readers.contains_key(&entry.chunk_number) {
            let chunk_file = self
                .chunks_dir
                .join(format!("chunk_{}.bin.zst", entry.chunk_number));
            if !chunk_file.exists() {
                anyhow::bail!(
                    "Chunk {} not found: {}",
                    entry.chunk_number,
                    chunk_file.display()
                );
            }

            let zstd_threads = std::cmp::min(6, num_cpus::get().saturating_sub(2));
            let mut zstd_proc = std::process::Command::new("zstd")
                .arg("-d")
                .arg("--stdout")
                .arg("-q")
                .arg(format!("-T{}", zstd_threads))
                .arg(&chunk_file)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .with_context(|| {
                    format!("Failed to start zstd for chunk {}", entry.chunk_number)
                })?;

            let stdout = zstd_proc
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
            let reader = std::io::BufReader::with_capacity(128 * 1024 * 1024, stdout);
            let offset = 0u64;

            readers.insert(entry.chunk_number, (reader, zstd_proc, offset));
        }

        // Now get the reader (we know it exists)
        let (reader, _proc, current_offset) = readers.get_mut(&entry.chunk_number).unwrap();

        // Seek to block offset if needed
        if *current_offset < entry.offset_in_chunk {
            let skip_bytes = entry.offset_in_chunk - *current_offset;
            const SKIP_BUF_SIZE: u64 = 64 * 1024 * 1024;
            let mut skip_buf = vec![0u8; (skip_bytes.min(SKIP_BUF_SIZE)) as usize];
            let mut remaining = skip_bytes;

            use std::io::Read;
            while remaining > 0 {
                let to_read = remaining.min(skip_buf.len() as u64) as usize;
                let bytes_read = reader.read(&mut skip_buf[..to_read]).with_context(|| {
                    format!(
                        "Failed to seek to offset {} in chunk {}",
                        entry.offset_in_chunk, entry.chunk_number
                    )
                })?;
                if bytes_read == 0 {
                    anyhow::bail!(
                        "Unexpected EOF while seeking in chunk {}",
                        entry.chunk_number
                    );
                }
                remaining -= bytes_read as u64;
            }
            *current_offset = entry.offset_in_chunk;
        } else if *current_offset > entry.offset_in_chunk {
            // Can't seek backwards - would need to restart chunk, but this should be rare
            // For now, just fail (could optimize by restarting chunk if needed)
            anyhow::bail!(
                "Cannot seek backwards in chunk {} (current={}, needed={})",
                entry.chunk_number,
                *current_offset,
                entry.offset_in_chunk
            );
        }

        // Read block length
        let mut len_buf = [0u8; 4];
        use std::io::Read;
        reader
            .read_exact(&mut len_buf)
            .with_context(|| format!("Failed to read block length at height {}", height))?;
        *current_offset += 4;

        let block_len = u32::from_le_bytes(len_buf) as usize;
        if block_len > 10 * 1024 * 1024 || block_len < 88 {
            anyhow::bail!(
                "Invalid block size: {} bytes (height {})",
                block_len,
                height
            );
        }

        // Read block data
        let mut block_data = vec![0u8; block_len];
        reader
            .read_exact(&mut block_data)
            .with_context(|| format!("Failed to read block data at height {}", height))?;
        *current_offset += block_len as u64;

        Ok(Some(block_data))
    }
}
