//! Materialize full `utxo_H.bin` snapshots from an existing base + delta chain.
//!
//! Usage:
//!   materialize_utxo_snapshots --dir <DIR>
//!
//! Scans `<DIR>` for `utxo_*.bin` bases and `delta_*.bin` chains, then writes every
//! `utxo_H.bin` that is reachable by applying deltas from an existing base but is
//! not yet on disk.
//!
//! The delta chain is followed in order: each delta's `base_height` must equal the
//! height of the current UTXO set.  Whenever the current height has no on-disk
//! `utxo_<current_height>.bin`, one is written before continuing.

use anyhow::{bail, Context, Result};
use blvm_bench::utxo_delta::{apply_delta, read_delta};
use blvm_bench::utxo_snapshot_fixed_v1::{
    decode_fixed_v1_file, encode_fixed_v1_unsorted_to_writer,
};
use clap::Parser;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "materialize_utxo_snapshots")]
struct Args {
    /// Directory containing `utxo_*.bin` and `delta_*.bin` files.
    #[arg(long, env = "UTXO_CHECKPOINT_FULL_DIR")]
    dir: PathBuf,

    /// Only materialise snapshots at heights that are multiples of this step.
    /// 0 = materialise every reachable height (default: 100000).
    #[arg(long, default_value_t = 100000)]
    step: u64,

    /// Dry-run: print what would be written without doing it.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let dir = &args.dir;

    // ── Collect existing utxo_H.bin heights ──────────────────────────────────
    let mut bases: BTreeMap<u64, PathBuf> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("open dir {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix("utxo_") {
            if let Some(h_str) = rest.strip_suffix(".bin") {
                if let Ok(h) = h_str.parse::<u64>() {
                    bases.insert(h, entry.path());
                }
            }
        }
    }
    eprintln!("Found {} existing utxo_*.bin:", bases.len());
    for (h, p) in &bases {
        eprintln!("  utxo_{h}.bin  ({})", p.display());
    }

    // ── Collect delta files: group by base_height ─────────────────────────────
    // delta_H.bin: base_height → H
    let mut deltas_from: BTreeMap<u64, Vec<(u64, PathBuf)>> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("open dir {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix("delta_") {
            if let Some(h_str) = rest.strip_suffix(".bin") {
                if let Ok(h) = h_str.parse::<u64>() {
                    // Read base_height from header (offset 20, 8 bytes LE u64).
                    let path = entry.path();
                    let base_h = read_delta_base_height(&path)
                        .with_context(|| format!("reading header of {}", path.display()))?;
                    deltas_from.entry(base_h).or_default().push((h, path));
                }
            }
        }
    }
    // Sort each group by target height ascending.
    for v in deltas_from.values_mut() {
        v.sort_by_key(|(h, _)| *h);
    }

    // ── Walk each base and follow its delta chain ─────────────────────────────
    let mut written = 0usize;
    for (base_h, base_path) in &bases {
        let Some(chain) = deltas_from.get(base_h) else {
            // No deltas rooted at this base — nothing to materialize from here.
            continue;
        };

        eprintln!(
            "\nBase utxo_{base_h}.bin → chain of {} delta(s)",
            chain.len()
        );

        // Build the contiguous chain: each delta's base must equal previous height.
        let mut current_height = *base_h;
        let mut set_loaded = false;
        let mut set = blvm_protocol::types::UtxoSet::default();

        // Walk only the strictly-chained deltas (stop if chain breaks).
        let mut remaining = chain.as_slice();
        while let Some(((target_h, delta_path), rest)) = remaining.split_first() {
            // Verify chain continuity (defensive; read_delta also checks inside
            // load_base_and_apply_deltas but we do it here so we can skip broken tails).
            let delta = read_delta(delta_path)
                .with_context(|| format!("reading {}", delta_path.display()))?;
            if delta.base_height != current_height {
                eprintln!(
                    "  ⚠️  Chain break at delta_{target_h}.bin: expected base={current_height}, got {}. Stopping this branch.",
                    delta.base_height
                );
                break;
            }

            // Lazy-load the base only when we actually need to apply a delta.
            if !set_loaded {
                eprintln!("  Loading base utxo_{base_h}.bin …");
                set = decode_fixed_v1_file(base_path)
                    .with_context(|| format!("decode {}", base_path.display()))?;
                set_loaded = true;
                eprintln!("  Loaded {} entries.", set.len());
            }

            eprintln!(
                "  Applying delta_{target_h}.bin (base={}, +{} -{})",
                delta.base_height,
                delta.added.len(),
                delta.removed.len()
            );
            apply_delta(&mut set, &delta);
            current_height = *target_h;

            // Write utxo_<current_height>.bin if it doesn't exist and step matches.
            let need_step = args.step == 0 || current_height % args.step == 0;
            let dest = dir.join(format!("utxo_{current_height}.bin"));
            if need_step && !bases.contains_key(&current_height) {
                if args.dry_run {
                    eprintln!(
                        "  [dry-run] would write {} ({} entries)",
                        dest.display(),
                        set.len()
                    );
                } else {
                    eprintln!("  Writing {} ({} entries) …", dest.display(), set.len());
                    write_fixed_v1_file(current_height, &set, &dest)
                        .with_context(|| format!("writing {}", dest.display()))?;
                    eprintln!("  ✅ Wrote {}", dest.display());
                    written += 1;
                }
            } else if need_step {
                eprintln!("  ✓ utxo_{current_height}.bin already exists, skipping.");
            }

            remaining = rest;
        }
    }

    if args.dry_run {
        eprintln!("\nDry-run complete.");
    } else {
        eprintln!("\nDone. Wrote {written} new snapshot(s).");
    }
    Ok(())
}

fn read_delta_base_height(path: &Path) -> Result<u64> {
    use std::io::Read;
    let mut f = File::open(path)?;
    let mut buf = [0u8; 28]; // magic(8) + version(4) + height(8) + base_height(8)
    f.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf[20..28].try_into().unwrap()))
}

fn write_fixed_v1_file(
    height: u64,
    set: &blvm_protocol::types::UtxoSet,
    dest: &Path,
) -> Result<()> {
    let tmp = dest.with_extension("bin.tmp");
    {
        let f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create tmp {}", tmp.display()))?;
        let mut w = BufWriter::with_capacity(4 * 1024 * 1024, f);
        encode_fixed_v1_unsorted_to_writer(height, set, &mut w)
            .with_context(|| format!("encode {}", dest.display()))?;
    }
    std::fs::rename(&tmp, dest)
        .with_context(|| format!("rename {} → {}", tmp.display(), dest.display()))?;
    Ok(())
}
