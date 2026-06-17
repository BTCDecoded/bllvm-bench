//! Resolve a fixed-v1 BLVM `UtxoSet` at height **H** from **`utxo_0.bin`** plus the contiguous
//! **`delta_*` chain** rooted at height 0 (same layout as **`materialize_utxo_snapshots`** /
//! **`chunk_utxo_checkpoints --delta-checkpoints`**).
//!
//! Optional **`utxo_H.bin`** one-file load exists for speed or interoperability; the canonical
//! ladder is **base + deltas**. **`block_kernel_diff`** uses the delta ladder by default so BLVM
//! matches the same artifacts you generated; Core still consumes **chainstate** (or a one-shot full
//! fixed-v1 import) until the fork can apply these deltas.

use anyhow::{Context, Result, bail};
use blvm_protocol::types::UtxoSet;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Read `base_height` from a `delta_*.bin` header (offset 20, 8 bytes LE).
pub fn read_delta_base_height(path: &Path) -> Result<u64> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = [0u8; 28];
    f.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf[20..28].try_into().unwrap()))
}

fn collect_deltas_by_base(dir: &Path) -> Result<BTreeMap<u64, Vec<(u64, PathBuf)>>> {
    let mut m: BTreeMap<u64, Vec<(u64, PathBuf)>> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("delta_") else {
            continue;
        };
        let Some(h_str) = rest.strip_suffix(".bin") else {
            continue;
        };
        let Ok(target_h) = h_str.parse::<u64>() else {
            continue;
        };
        let path = entry.path();
        let base = read_delta_base_height(&path)
            .with_context(|| format!("reading delta header {}", path.display()))?;
        m.entry(base).or_default().push((target_h, path));
    }
    for v in m.values_mut() {
        v.sort_by_key(|(h, _)| *h);
    }
    Ok(m)
}

/// Reconstruct **fixed-v1** UTXO state after block **`height`** using **`utxo_0.bin`** and the
/// contiguous **`delta_*`** chain, walking transitively through base heights
/// (0 → 100k → 200k → …) until the target is reached.
pub fn load_fixed_v1_utxo_via_deltas(checkpoint_dir: &Path, height: u64) -> Result<UtxoSet> {
    let base_path = checkpoint_dir.join("utxo_0.bin");
    if !base_path.is_file() {
        bail!("missing utxo_0.bin under {}", checkpoint_dir.display());
    }

    let deltas_by_base = collect_deltas_by_base(checkpoint_dir)?;

    let mut set = crate::utxo_snapshot_fixed_v1::decode_fixed_v1_file(&base_path)
        .with_context(|| format!("decode {}", base_path.display()))?;
    let mut current = crate::utxo_snapshot_fixed_v1::read_header_height(&base_path)?;
    if current != 0 {
        bail!(
            "{} header height is {} (expected 0)",
            base_path.display(),
            current
        );
    }

    while current < height {
        let chain = deltas_by_base.get(&current).with_context(|| {
            format!(
                "delta chain under {} has no delta rooted at height {} (need to reach {})",
                checkpoint_dir.display(),
                current,
                height
            )
        })?;
        // Pick the highest delta that does not overshoot.
        let (target_h, path) = chain
            .iter()
            .rev()
            .find(|(h, _)| *h <= height)
            .with_context(|| {
                format!(
                    "no delta from base {} reaches ≤ {} under {}",
                    current,
                    height,
                    checkpoint_dir.display()
                )
            })?;
        let delta = crate::utxo_delta::read_delta(path)?;
        if delta.base_height != current {
            bail!(
                "delta chain broken at {}: expected base_height {}, got {}",
                path.display(),
                current,
                delta.base_height
            );
        }
        eprintln!(
            "   📦 applying {} (base {} → target {})",
            path.display(),
            current,
            target_h
        );
        crate::utxo_delta::apply_delta(&mut set, &delta);
        current = *target_h;
    }

    if current != height {
        bail!(
            "internal error: delta walk landed at {} != target {}",
            current,
            height
        );
    }

    Ok(set)
}

/// Load UTXO at **`height`**: **`utxo_{height}.bin`** (fixed-v1) if present, else **[`load_fixed_v1_utxo_via_deltas`]**.
///
/// For **`materialize_utxo_snapshots`**, tooling, or **`--blvm-prefer-utxo-snapshot`**.
pub fn load_fixed_v1_utxo_at_height(checkpoint_dir: &Path, height: u64) -> Result<UtxoSet> {
    let full = checkpoint_dir.join(format!("utxo_{height}.bin"));
    if full.is_file() {
        let h_file = crate::utxo_snapshot_fixed_v1::read_header_height(&full)?;
        if h_file != height {
            bail!(
                "{} header height {} != requested {}",
                full.display(),
                h_file,
                height
            );
        }
        return crate::utxo_snapshot_fixed_v1::decode_fixed_v1_file(&full)
            .with_context(|| format!("decode {}", full.display()));
    }

    load_fixed_v1_utxo_via_deltas(checkpoint_dir, height)
}
