//! Chunk cache paths from environment only — never hardcode machine-specific directories.
//!
//! Set `BLOCK_CACHE_DIR` in `.env` (see repository `.env.example`) or in the shell.

use std::path::{Path, PathBuf};

fn non_empty_env_triple(primary: &str, alt: &str, legacy: &str) -> bool {
    [primary, alt, legacy].iter().any(|k| {
        std::env::var(k)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    })
}

/// True when remote-Core SSH/RPC env is set (`REMOTE_CORE_*`, or legacy `LAND_NODE_*` / `START9_*`).
pub fn remote_core_rpc_env_ready() -> bool {
    non_empty_env_triple("REMOTE_CORE_SSH_KEY", "LAND_NODE_SSH_KEY", "START9_SSH_KEY")
        && non_empty_env_triple(
            "REMOTE_CORE_SSH_HOST",
            "LAND_NODE_SSH_HOST",
            "START9_SSH_HOST",
        )
        && non_empty_env_triple(
            "REMOTE_CORE_RPC_USER",
            "LAND_NODE_RPC_USER",
            "START9_RPC_USER",
        )
        && non_empty_env_triple(
            "REMOTE_CORE_RPC_PASSWORD",
            "LAND_NODE_RPC_PASSWORD",
            "START9_RPC_PASSWORD",
        )
}

/// XOR-packaged / out-of-order `blk*.dat` trees: set `REMOTE_CORE_XOR_BLOCKFILES=1` (or `true`),
/// or legacy `LAND_NODE_XOR_BLOCKFILES`, or path substring `bitcoin-start9` under `path`.
pub fn remote_core_xor_blockfiles_hint(path: &Path) -> bool {
    for key in ["REMOTE_CORE_XOR_BLOCKFILES", "LAND_NODE_XOR_BLOCKFILES"] {
        if std::env::var(key)
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
        {
            return true;
        }
    }
    path.to_string_lossy().contains("bitcoin-start9")
}

/// Cache file for reordered block stream when XOR / out-of-order packaging is used.
pub fn remote_core_ordered_blocks_cache_basename() -> &'static str {
    "remote_core_ordered_blocks.bin"
}

/// Basenames to try when opening an existing ordered-blocks cache (newest name first).
pub fn remote_core_ordered_blocks_cache_basenames() -> [&'static str; 3] {
    [
        "remote_core_ordered_blocks.bin",
        "land_node_ordered_blocks.bin",
        "start9_ordered_blocks.bin",
    ]
}

/// Bitcoin Core **data directories** to try for direct `blk*.dat` access (`blocks/` must exist under each).
///
/// Order:
/// 1. `BITCOIN_DATA_DIR` if set and non-empty (same as `fill_missing_from_files` and other bench tools).
/// 2. Entries from `BITCOIN_DATA_DIRS` (platform path-list, e.g. multiple paths separated like `PATH`).
///
/// Empty when neither is set — callers should fall back to chunk cache or RPC.
pub fn bitcoin_data_dir_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    if let Ok(p) = std::env::var("BITCOIN_DATA_DIR") {
        let pb = PathBuf::from(p.trim());
        if !pb.as_os_str().is_empty() {
            out.push(pb);
        }
    }

    if let Ok(raw) = std::env::var("BITCOIN_DATA_DIRS") {
        if !raw.trim().is_empty() {
            for pb in std::env::split_paths(&raw) {
                if !pb.as_os_str().is_empty() && !out.iter().any(|e| e == &pb) {
                    out.push(pb);
                }
            }
        }
    }

    out
}

/// Chunk cache root from `BLOCK_CACHE_DIR` if set and non-empty.
pub fn block_cache_dir_from_env() -> Option<PathBuf> {
    let v = std::env::var_os("BLOCK_CACHE_DIR")?;
    if v.is_empty() {
        return None;
    }
    Some(PathBuf::from(v))
}

/// Same as [`block_cache_dir_from_env`] but requires the path to exist.
pub fn require_block_cache_dir() -> anyhow::Result<PathBuf> {
    let p = block_cache_dir_from_env().ok_or_else(|| {
        anyhow::anyhow!(
            "BLOCK_CACHE_DIR is not set or empty. Set it to your local chunk cache root (copy `.env.example` to `.env`)."
        )
    })?;
    if !p.exists() {
        anyhow::bail!("BLOCK_CACHE_DIR does not exist: {}", p.display());
    }
    Ok(p)
}

/// `BLOCK_CACHE_DIR` joined with `relative` (e.g. `sort_merge_data`).
pub fn require_block_cache_subdir(relative: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    Ok(require_block_cache_dir()?.join(relative))
}

/// Sort-merge working directory: `SORT_MERGE_DIR`, or `{BLOCK_CACHE_DIR}/sort_merge_data`.
pub fn sort_merge_data_dir() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("SORT_MERGE_DIR") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    Ok(require_block_cache_dir()?.join("sort_merge_data"))
}

/// Raw Bitcoin `.blk` tree for tools that read Core block files (e.g. `recollect_blocks`).
pub fn require_bitcoin_blk_dir() -> anyhow::Result<PathBuf> {
    let p = std::env::var_os("BITCOIN_BLK_DIR").ok_or_else(|| {
        anyhow::anyhow!(
            "BITCOIN_BLK_DIR is not set. Set it to the directory containing Bitcoin Core blk*.dat files."
        )
    })?;
    if p.is_empty() {
        anyhow::bail!("BITCOIN_BLK_DIR is empty");
    }
    let pb = PathBuf::from(p);
    if !pb.exists() {
        anyhow::bail!("BITCOIN_BLK_DIR does not exist: {}", pb.display());
    }
    Ok(pb)
}
