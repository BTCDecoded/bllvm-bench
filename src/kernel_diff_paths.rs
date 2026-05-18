//! Default paths for the kernel differential harness (`block_kernel_diff`).
//!
//! ## Local env (LAN RPC, paths)
//! Call [`load_kernel_diff_env`] **before** `clap` parses so `env = "..."` args see `BITCOIN_RPC_*`.
//! First file that exists wins: `KERNEL_DIFF_ENV_FILE`, then `kernel-diff.local.env`,
//! `blvm-bench/kernel-diff.local.env`, `.env.local`, `.env`. Those files are gitignored — copy from
//! `.env.example`.
//!
//! Root: **`~/.local/share/blvm-kernel-diff/`** (XDG [`dirs::data_local_dir`] or `~/.local/share`).
//!
//! | Subdirectory | Purpose |
//! |--------------|---------|
//! | **`core-datadir/`** | Default **`CORE_DIFF_DATADIR`** for libbitcoinkernel (chainstate + `blocks/`) |
//! | **`chunk-cache/`** | Optional: symlink your real chunk cache here if you do not set `--block-cache-dir` |
//!
//! You can still override everything with **`--block-cache-dir`**, **`--core-datadir`**, or env vars.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Load optional env vars for kernel differential runs (non-fatal if no file exists).
/// See module docs for search order.
pub fn load_kernel_diff_env() {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("KERNEL_DIFF_ENV_FILE") {
        if !p.trim().is_empty() {
            candidates.push(PathBuf::from(p));
        }
    }
    candidates.push(PathBuf::from("kernel-diff.local.env"));
    candidates.push(PathBuf::from("blvm-bench/kernel-diff.local.env"));
    candidates.push(PathBuf::from(".env.local"));
    candidates.push(PathBuf::from(".env"));

    for path in candidates {
        if path.is_file() {
            match dotenvy::from_path(&path) {
                Ok(()) => {
                    eprintln!("   📄 Loaded kernel-diff env from {}", path.display());
                    return;
                }
                Err(e) => {
                    eprintln!("   ⚠️  Could not load {}: {}", path.display(), e);
                }
            }
        }
    }
}

/// `XDG_DATA_HOME/blvm-kernel-diff` or `~/.local/share/blvm-kernel-diff`.
pub fn kernel_diff_data_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/share")
        })
        .join("blvm-kernel-diff")
}

/// Default libbitcoinkernel datadir: **`kernel_diff_data_root()/core-datadir`**.
pub fn kernel_diff_default_core_datadir() -> PathBuf {
    kernel_diff_data_root().join("core-datadir")
}

/// Default chunk cache *hint* path (used only if it contains `chunks.meta` or `chunks/chunks.meta`).
pub fn kernel_diff_default_chunk_cache_dir() -> PathBuf {
    kernel_diff_data_root().join("chunk-cache")
}

fn chunk_cache_looks_valid(root: &Path) -> bool {
    root.join("chunks.meta").exists() || root.join("chunks").join("chunks.meta").exists()
}

/// Directory that contains **`chunks.meta`** — either **`cache_root`** (flat) or **`cache_root/chunks/`** (nested).
pub fn resolve_chunks_data_dir(cache_root: &Path) -> Result<PathBuf> {
    if cache_root.join("chunks.meta").exists() {
        return Ok(cache_root.to_path_buf());
    }
    let nested = cache_root.join("chunks");
    if nested.join("chunks.meta").exists() {
        return Ok(nested);
    }
    anyhow::bail!(
        "chunk cache not found: need chunks.meta at {} or {}",
        cache_root.display(),
        nested.display()
    );
}

/// Resolve chunk cache root for `block_kernel_diff`.
///
/// Order: **`--block-cache-dir`** > **`BLOCK_CACHE_DIR`** env > default **`chunk-cache/`** if layout valid >
/// > (with **`scan`** / **`differential`** feature) XDG chunk dir from `get_chunks_dir`.
pub fn resolve_block_cache_root(cli_override: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = cli_override {
        return Ok(p);
    }
    if let Some(p) = crate::block_cache_env::block_cache_dir_from_env() {
        return Ok(p);
    }
    let def = kernel_diff_default_chunk_cache_dir();
    if chunk_cache_looks_valid(&def) {
        return Ok(def);
    }
    #[cfg(any(feature = "differential", feature = "scan"))]
    if let Some(p) = crate::chunked_cache::get_chunks_dir() {
        return Ok(p);
    }
    anyhow::bail!(
        "No chunk cache found. Point at a directory with chunks.meta:\n\
         • Symlink: ln -s /your/chunk/cache {}\n\
         • Or: --block-cache-dir /path\n\
         • Or: export BLOCK_CACHE_DIR=/path",
        def.display()
    );
}

/// `network_time` for `blvm_protocol::block::block_validation_context_for_connect_ibd` (or
/// `BlockValidationContext::from_connect_block_ibd_args` with BIP54 args `None`)
/// when replaying **historical** blocks from the chunk cache.
///
/// Core’s assume-valid path requires `block_timestamp + 2 weeks <= network_time` (`two_week_ok`).
/// Using the block’s own timestamp as `network_time` makes that check **always false**, so signature
/// skipping never activates even when `BLVM_ASSUME_VALID_HEIGHT` is set.
#[must_use]
pub fn network_time_for_historical_chunk_replay(block_timestamp: u64) -> u64 {
    const TWO_WEEKS_SECS: u64 = 2 * 7 * 24 * 3600;
    block_timestamp.saturating_add(TWO_WEEKS_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_root_ends_with_blvm_kernel_diff() {
        let r = kernel_diff_data_root();
        assert!(r.ends_with("blvm-kernel-diff"));
    }
}
