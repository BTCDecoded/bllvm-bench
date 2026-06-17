//! Load/save BLVM UTXO checkpoints under `{cache_root}/<subdir>/` (default **`differential_checkpoints/`**).
//!
//! - **Bincode** (default): `HashMap<OutPoint, UTXO>` via `bincode` (values without `Arc`).
//! - **Fixed v1:** see **`docs/UTXO_SNAPSHOT_FIXED_V1.md`** and **`utxo_snapshot_fixed_v1`**.
//!
//! Both formats write to a same-directory **`*.part`** file, then **`rename`** to `utxo_H.bin` (atomic replace).
//!
//! **`utxo_H.bin`** is the UTXO set **after** successfully connecting block **`H`**.
//!
//! **Load** autodetects: magic `BLVMUX01` → fixed v1 (streamed decode — no full-file `read` buffer);
//! otherwise bincode (whole file in memory).
//!
//! After a successful write, the file is marked **read-only** to reduce accidental overwrites.
//! On Unix, `rm` can still remove the file if the **parent directory** is writable.

use anyhow::{Context, Result};
use blvm_protocol::types::{OutPoint, UTXO, UtxoSet};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Write via a temp file next to `path`, then [`std::fs::rename`] so readers never see a half-written checkpoint.
fn write_checkpoint_temp_rename(
    path: &Path,
    height: u64,
    write_body: impl FnOnce(File) -> Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = parent.join(format!(
        ".utxo_{}_{}_{}.part",
        height,
        std::process::id(),
        nanos
    ));

    let write_result = (|| -> Result<()> {
        let file = File::create(&tmp_path)
            .with_context(|| format!("create temp {}", tmp_path.display()))?;
        write_body(file)
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    write_result?;

    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;
    Ok(())
}

/// On-disk checkpoint encoding for **writes** (`--checkpoint-every`, exports). **Loads** always autodetect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum CheckpointFormat {
    #[default]
    #[value(name = "bincode")]
    Bincode,
    #[value(name = "fixed-v1")]
    FixedV1,
}

/// Access UTXO checkpoint files next to a chunk cache root.
pub struct CheckpointManager {
    cache_root: PathBuf,
    /// Directory under `cache_root` (e.g. `differential_checkpoints` or `differential_checkpoints_fixed_v1`).
    checkpoint_subdir: PathBuf,
}

impl CheckpointManager {
    pub fn new(cache_root: impl AsRef<Path>) -> Result<Self> {
        Self::with_checkpoint_subdir(cache_root, "differential_checkpoints")
    }

    /// Same as [`Self::new`] but uses a custom subdirectory under `cache_root` (must be a relative path).
    pub fn with_checkpoint_subdir(
        cache_root: impl AsRef<Path>,
        subdir: impl AsRef<Path>,
    ) -> Result<Self> {
        let subdir = subdir.as_ref();
        if subdir.as_os_str().is_empty() || subdir.is_absolute() {
            anyhow::bail!(
                "checkpoint subdir must be non-empty and relative (got {})",
                subdir.display()
            );
        }
        Ok(Self {
            cache_root: cache_root.as_ref().to_path_buf(),
            checkpoint_subdir: subdir.to_path_buf(),
        })
    }

    fn checkpoint_path(&self, height: u64) -> PathBuf {
        self.cache_root
            .join(&self.checkpoint_subdir)
            .join(format!("utxo_{}.bin", height))
    }

    /// Load `utxo_{height}.bin` if present. **Autodetect:** fixed v1 (magic `BLVMUX01`) or legacy bincode.
    pub fn load_utxo_checkpoint(&self, height: u64) -> Result<Option<UtxoSet>> {
        let path = self.checkpoint_path(height);
        if !path.is_file() {
            return Ok(None);
        }
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)
            .with_context(|| format!("read magic {}", path.display()))?;

        if magic == *crate::utxo_snapshot_fixed_v1::FIXED_V1_MAGIC {
            file.seek(std::io::SeekFrom::Start(0))
                .with_context(|| format!("seek start {}", path.display()))?;
            let br = std::io::BufReader::with_capacity(1024 * 1024, file);
            let set = crate::utxo_snapshot_fixed_v1::decode_fixed_v1_reader(br)
                .with_context(|| format!("fixed-v1 decode {}", path.display()))?;
            return Ok(Some(set));
        }

        let mut data = magic.to_vec();
        file.read_to_end(&mut data)
            .with_context(|| format!("read body {}", path.display()))?;

        let raw: HashMap<OutPoint, UTXO> = bincode::deserialize(&data)
            .with_context(|| format!("bincode deserialize UTXO checkpoint {}", path.display()))?;
        let set: UtxoSet = raw.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
        Ok(Some(set))
    }

    /// Write `utxo_{height}.bin` (UTXO state **after** block `height`) using `format`.
    pub fn save_utxo_checkpoint(
        &self,
        height: u64,
        utxo: &UtxoSet,
        format: CheckpointFormat,
    ) -> Result<()> {
        let path = self.checkpoint_path(height);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all {}", parent.display()))?;
        }
        if path.exists() {
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms)
                .with_context(|| format!("chmod +w before overwrite {}", path.display()))?;
        }

        match format {
            CheckpointFormat::Bincode => {
                let map: HashMap<OutPoint, UTXO> =
                    utxo.iter().map(|(k, v)| (*k, (**v).clone())).collect();
                write_checkpoint_temp_rename(&path, height, |file| {
                    let mut w = BufWriter::with_capacity(1024 * 1024, file);
                    bincode::serialize_into(&mut w, &map)
                        .with_context(|| format!("serialize UTXO checkpoint {}", path.display()))?;
                    w.flush()
                        .with_context(|| format!("flush temp bincode {}", path.display()))?;
                    let file = w
                        .into_inner()
                        .map_err(|e| anyhow::anyhow!("BufWriter finalize: {e}"))?;
                    let _ = file.sync_all();
                    Ok(())
                })?;
            }
            CheckpointFormat::FixedV1 => {
                let low_mem = matches!(
                    std::env::var("CHUNK_UTXO_LOW_MEM").as_deref(),
                    Ok("1") | Ok("true")
                );
                write_checkpoint_temp_rename(&path, height, |file| {
                    let mut bw = BufWriter::with_capacity(1024 * 1024, file);
                    if low_mem {
                        crate::utxo_snapshot_fixed_v1::encode_fixed_v1_unsorted_to_writer(
                            height, utxo, &mut bw,
                        )
                    } else {
                        crate::utxo_snapshot_fixed_v1::encode_fixed_v1_to_writer(
                            height, utxo, &mut bw,
                        )
                    }
                    .with_context(|| format!("encode fixed-v1 {}", path.display()))?;
                    bw.flush()
                        .with_context(|| format!("flush temp fixed-v1 {}", path.display()))?;
                    let file = bw
                        .into_inner()
                        .map_err(|e| anyhow::anyhow!("BufWriter finalize: {e}"))?;
                    let _ = file.sync_all();
                    Ok(())
                })?;
            }
        }

        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms)
            .with_context(|| format!("set read-only {}", path.display()))?;
        Ok(())
    }

    /// Delete old `utxo_*.bin` files, keeping only the `keep` most recent by height.
    /// Skips files whose name doesn't match `utxo_<digits>.bin`.
    pub fn prune_old_checkpoints(&self, keep: usize) -> Result<usize> {
        let dir = self.cache_root.join(&self.checkpoint_subdir);
        if !dir.is_dir() {
            return Ok(0);
        }
        let mut heights: Vec<u64> = Vec::new();
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(rest) = name_str.strip_prefix("utxo_") {
                if let Some(digits) = rest.strip_suffix(".bin") {
                    if let Ok(h) = digits.parse::<u64>() {
                        heights.push(h);
                    }
                }
            }
        }
        heights.sort_unstable();
        let to_delete = heights.len().saturating_sub(keep);
        let mut deleted = 0usize;
        for &h in heights.iter().take(to_delete) {
            let p = self.checkpoint_path(h);
            if p.is_file() {
                let mut perms = std::fs::metadata(&p)?.permissions();
                perms.set_readonly(false);
                std::fs::set_permissions(&p, perms).ok();
                std::fs::remove_file(&p)
                    .with_context(|| format!("remove old checkpoint {}", p.display()))?;
                deleted += 1;
            }
        }
        if deleted > 0 {
            eprintln!("   [retention] deleted {deleted} old checkpoint(s), kept newest {keep}");
        }
        Ok(deleted)
    }
}
