//! Fixed binary layout **v1** for differential UTXO checkpoints (bench-only; not consensus wire format).
//!
//! Normative documentation: **`docs/UTXO_SNAPSHOT_FIXED_V1.md`**.

use anyhow::{bail, Context, Result};
use blvm_protocol::types::{utxo_set_with_capacity, OutPoint, UtxoSet, UTXO};
use std::cmp::Ordering;
use std::io::{Read, Write};
use std::ops::Deref;
use std::sync::Arc;

/// Magic bytes; must match [`decode_fixed_v1_if_magic`].
pub const FIXED_V1_MAGIC: &[u8; 8] = b"BLVMUX01";
pub const FIXED_V1_FORMAT_VERSION: u32 = 1;

/// Max `script_pubkey` length in fixed-v1 / delta payloads (decoder sanity bound).
///
/// Consensus **`MAX_SCRIPT_SIZE`** (execution limit) is **10_000**. Mainnet still
/// accepts larger **output** `script_pubkey` bytes in mined blocks. Observed examples: ~18 KiB
/// (~869k), ~980 KiB (~897k). Use a generous cap so we do not trip mid-run; on-disk length is
/// still `u32` (max 4 GiB — do not raise that high without thought on RAM for malicious files).
pub const MAX_SCRIPT_PUBKEY_BYTES: usize = 16 * 1024 * 1024;

/// Ensure every valid UTXO in `set` fits the fixed-v1 / delta wire format **before** mutating
/// disk-backed state for this block. Otherwise `apply_block_delta` could succeed and a later
/// `record_block` / `finalize_to_file` read would fail, making resume height misleading.
pub fn check_utxoset_checkpointable(set: &UtxoSet) -> Result<()> {
    for (op, u) in set.iter() {
        let n = u.script_pubkey.len();
        if n > MAX_SCRIPT_PUBKEY_BYTES {
            bail!(
                "checkpoint format: script_pubkey length {n} bytes exceeds max {} at {op:?}; raise MAX_SCRIPT_PUBKEY_BYTES in utxo_snapshot_fixed_v1 if mainnet requires it",
                MAX_SCRIPT_PUBKEY_BYTES
            );
        }
    }
    Ok(())
}

/// Total fixed-v1 header size on disk (magic + version + height + count).
pub const HEADER_LEN: usize = 8 + 4 + 8 + 8;

/// Use parallel sort when there are at least this many UTXOs (sorts keys only; values stay in the map).
const PAR_SORT_KEYS_THRESHOLD: usize = 8_192;

/// Parallel key-sort during fixed-v1 **encode** spikes RAM on huge sets. Serial-only when set (e.g. low-RAM hosts).
fn fixed_v1_encode_use_serial_sort_only() -> bool {
    matches!(
        std::env::var("CHUNK_UTXO_SERIAL_ENCODE").as_deref(),
        Ok("1") | Ok("true")
    ) || matches!(
        std::env::var("CHUNK_UTXO_LOW_MEM").as_deref(),
        Ok("1") | Ok("true")
    )
}

#[inline]
fn cmp_outpoint_lex(a: &OutPoint, b: &OutPoint) -> Ordering {
    a.hash
        .as_slice()
        .cmp(b.hash.as_slice())
        .then_with(|| a.index.cmp(&b.index))
}

/// Encode the UTXO set **after** block `height` into an in-memory buffer (convenience; uses [`encode_fixed_v1_to_writer`]).
pub fn encode_fixed_v1(height: u64, utxo: &UtxoSet) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    encode_fixed_v1_to_writer(height, utxo, &mut out)?;
    Ok(out)
}

/// Stream fixed-v1 snapshot to a writer with **sorted** keys (deterministic output).
/// Allocates a `Vec<OutPoint>` (~36 bytes × N entries) for sorting. For low-RAM hosts, use
/// [`encode_fixed_v1_unsorted_to_writer`] instead (zero extra allocation).
pub fn encode_fixed_v1_to_writer<W: Write + ?Sized>(
    height: u64,
    utxo: &UtxoSet,
    w: &mut W,
) -> Result<()> {
    let mut keys: Vec<OutPoint> = utxo.keys().copied().collect();
    let n = keys.len();

    if n >= PAR_SORT_KEYS_THRESHOLD && !fixed_v1_encode_use_serial_sort_only() {
        use rayon::prelude::*;
        keys.par_sort_unstable_by(|a, b| cmp_outpoint_lex(a, b));
    } else {
        keys.sort_unstable_by(|a, b| cmp_outpoint_lex(a, b));
    }

    w.write_all(FIXED_V1_MAGIC.as_slice())?;
    w.write_all(&FIXED_V1_FORMAT_VERSION.to_le_bytes())?;
    w.write_all(&height.to_le_bytes())?;
    w.write_all(&(n as u64).to_le_bytes())?;

    for op in keys {
        let u = utxo
            .get(&op)
            .ok_or_else(|| anyhow::anyhow!("internal: missing outpoint after key collection"))?;
        write_fixed_v1_entry(w, &op, u.as_ref())?;
    }
    Ok(())
}

/// Stream fixed-v1 snapshot **without sorting** — iterates the HashMap directly.
/// **Zero extra allocation** beyond the writer buffer. Decoder builds a HashMap so order is irrelevant.
/// Prefer this on low-RAM hosts where the sorted `Vec<OutPoint>` (~1.8 GB at 50M entries) would OOM.
pub fn encode_fixed_v1_unsorted_to_writer<W: Write + ?Sized>(
    height: u64,
    utxo: &UtxoSet,
    w: &mut W,
) -> Result<()> {
    let n = utxo.len();
    w.write_all(FIXED_V1_MAGIC.as_slice())?;
    w.write_all(&FIXED_V1_FORMAT_VERSION.to_le_bytes())?;
    w.write_all(&height.to_le_bytes())?;
    w.write_all(&(n as u64).to_le_bytes())?;

    for (op, u) in utxo.iter() {
        write_fixed_v1_entry(w, op, u.as_ref())?;
    }
    Ok(())
}

fn write_fixed_v1_entry<W: Write + ?Sized>(w: &mut W, op: &OutPoint, u: &UTXO) -> Result<()> {
    let script = u.script_pubkey.deref();
    if script.len() > MAX_SCRIPT_PUBKEY_BYTES {
        bail!(
            "script_pubkey length {} exceeds max {}",
            script.len(),
            MAX_SCRIPT_PUBKEY_BYTES
        );
    }
    w.write_all(&op.hash)?;
    w.write_all(&op.index.to_le_bytes())?;
    w.write_all(&u.value.to_le_bytes())?;
    w.write_all(&u.height.to_le_bytes())?;
    w.write_all(&[u.is_coinbase as u8])?;
    w.write_all(&(script.len() as u32).to_le_bytes())?;
    w.write_all(script)?;
    Ok(())
}

/// Read one fixed-v1 entry from `r` (same layout as [`write_fixed_v1_entry`]).
fn read_one_entry<R: Read>(r: &mut R) -> Result<(OutPoint, UTXO)> {
    let mut hash = [0u8; 32];
    r.read_exact(&mut hash)?;
    let mut u4 = [0u8; 4];
    r.read_exact(&mut u4)?;
    let index = u32::from_le_bytes(u4);
    let mut u8b = [0u8; 8];
    r.read_exact(&mut u8b)?;
    let value = i64::from_le_bytes(u8b);
    r.read_exact(&mut u8b)?;
    let height = u64::from_le_bytes(u8b);
    let mut cb = [0u8; 1];
    r.read_exact(&mut cb)?;
    let is_coinbase = match cb[0] {
        0 => false,
        1 => true,
        b => bail!("fixed v1: invalid is_coinbase byte {}", b),
    };
    r.read_exact(&mut u4)?;
    let slen = u32::from_le_bytes(u4) as usize;
    if slen > MAX_SCRIPT_PUBKEY_BYTES {
        bail!(
            "fixed v1: script length {} exceeds max {}",
            slen,
            MAX_SCRIPT_PUBKEY_BYTES
        );
    }
    let mut script = vec![0u8; slen];
    r.read_exact(&mut script)?;
    let op = OutPoint { hash, index };
    let utxo = UTXO {
        value,
        script_pubkey: script.into(),
        height,
        is_coinbase,
    };
    Ok((op, utxo))
}

/// Decode fixed-v1 from a reader positioned at **file start** (reads full header + entries).
///
/// Peak memory is approximately the final [`UtxoSet`] plus one entry, **not** the full file size
/// (unlike loading the file into a `Vec` and decoding from a slice).
pub fn decode_fixed_v1_reader<R: Read>(mut r: R) -> Result<UtxoSet> {
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header)?;
    if header[..8] != FIXED_V1_MAGIC[..] {
        bail!("fixed v1: missing or wrong magic");
    }
    let format_ver = u32::from_le_bytes(
        header[8..12]
            .try_into()
            .map_err(|_| anyhow::anyhow!("fixed v1: format version"))?,
    );
    if format_ver != FIXED_V1_FORMAT_VERSION {
        bail!(
            "fixed v1: unsupported format version {} (expected {})",
            format_ver,
            FIXED_V1_FORMAT_VERSION
        );
    }
    let _snapshot_height = u64::from_le_bytes(header[12..20].try_into()?);
    let count = u64::from_le_bytes(header[20..28].try_into()?);
    let count_usize =
        usize::try_from(count).map_err(|_| anyhow::anyhow!("fixed v1: entry count overflow"))?;

    // Allocate the correct power-of-2 hashbrown table BEFORE inserting any Arc<UTXO>.
    //
    // Key points:
    //   - with_capacity(N) → hashbrown allocates ceil(N/0.875) slots rounded to next pow2
    //   - For N > 58_720_256 (= 67M × 0.875): triggers the 134M-slot table (6.57 GB)
    //   - For N > 117_440_513 (= 134M × 0.875): triggers the 268M-slot table (12.88 GB)
    //
    // We pre-size to avoid a mid-processing resize spike. With CHECKPOINT_EVERY=25_000 and
    // ~224 net UTXOs/block, one segment adds at most 5.6 M entries. Reserve for that growth
    // so the table won't resize during block processing.
    //
    // The memory-guard in chunk_utxo_checkpoints exits cleanly before the system OOMs,
    // so we don't need to worry about crossing the 134M→268M boundary mid-segment.
    let growth_reserve: usize = 6_000_000; // ~25k blocks × 240 net UTXOs/block, with margin
    let capacity_hint = if count_usize > 45_000_000 {
        // Force at least the 134M table so we start above the 67M resize threshold.
        // Add growth_reserve so the table also covers the full segment without mid-run resize.
        count_usize.saturating_add(growth_reserve).max(58_720_257)
    } else {
        count_usize.saturating_add(growth_reserve)
    };
    let mut set = utxo_set_with_capacity(capacity_hint);
    for _ in 0..count_usize {
        let (op, utxo) = read_one_entry(&mut r)?;
        set.insert(op, Arc::new(utxo));
    }

    let mut tail = [0u8; 1];
    match r.read(&mut tail) {
        Ok(0) => Ok(set),
        Ok(_) => bail!("fixed v1: trailing bytes after last entry"),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(set),
        Err(e) => Err(e.into()),
    }
}

fn decode_fixed_v1_body(data: &[u8]) -> Result<UtxoSet> {
    decode_fixed_v1_reader(std::io::Cursor::new(data))
}

/// If `data` starts with [`FIXED_V1_MAGIC`], decode and return `Some`; otherwise `Ok(None)`.
pub fn decode_fixed_v1_if_magic(data: &[u8]) -> Result<Option<UtxoSet>> {
    if data.len() < HEADER_LEN {
        return Ok(None);
    }
    if data[..8] != FIXED_V1_MAGIC[..] {
        return Ok(None);
    }
    decode_fixed_v1_body(data).map(Some)
}

/// Decode fixed v1; fails if magic missing or wrong.
pub fn decode_fixed_v1(data: &[u8]) -> Result<UtxoSet> {
    if data.len() < HEADER_LEN || data[..8] != FIXED_V1_MAGIC[..] {
        bail!("fixed v1: missing or wrong magic");
    }
    decode_fixed_v1_body(data)
}

/// Open a fixed-v1 file and decode the full UTXO set from it (convenience wrapper).
pub fn decode_fixed_v1_file(path: &std::path::Path) -> Result<UtxoSet> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let r = std::io::BufReader::with_capacity(4 * 1024 * 1024, file);
    decode_fixed_v1_reader(r).with_context(|| format!("decode {}", path.display()))
}

/// Read only the snapshot height from a fixed-v1 file header without loading UTXOs.
pub fn read_header_height(path: &std::path::Path) -> Result<u64> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut header = [0u8; HEADER_LEN];
    std::io::Read::read_exact(&mut file, &mut header)?;
    if header[..8] != FIXED_V1_MAGIC[..] {
        bail!("not a fixed-v1 file: {}", path.display());
    }
    Ok(u64::from_le_bytes(header[12..20].try_into()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blvm_protocol::types::UTXO;
    use std::sync::Arc;

    #[test]
    fn roundtrip_empty() {
        let set = UtxoSet::default();
        let bytes = encode_fixed_v1(0, &set).unwrap();
        let back = decode_fixed_v1(&bytes).unwrap();
        assert_eq!(back.len(), 0);
    }

    #[test]
    fn roundtrip_one() {
        let mut set = UtxoSet::default();
        let op = OutPoint {
            hash: [2u8; 32],
            index: 1,
        };
        set.insert(
            op,
            Arc::new(UTXO {
                value: 50_000_000,
                script_pubkey: vec![0x51].as_slice().into(),
                height: 100,
                is_coinbase: false,
            }),
        );
        let bytes = encode_fixed_v1(100, &set).unwrap();
        let back = decode_fixed_v1(&bytes).unwrap();
        assert_eq!(back.len(), 1);
        let u = back.get(&op).unwrap();
        assert_eq!(u.value, 50_000_000);
        assert_eq!(u.height, 100);
        assert!(!u.is_coinbase);
    }

    #[test]
    fn streaming_matches_vec_encoder() {
        let mut set = UtxoSet::default();
        for i in 0u32..50 {
            let op = OutPoint {
                hash: [i as u8; 32],
                index: i % 7,
            };
            set.insert(
                op,
                Arc::new(UTXO {
                    value: i as i64 * 1000,
                    script_pubkey: vec![0x51, (i & 0xff) as u8].into(),
                    height: u64::from(i),
                    is_coinbase: i % 2 == 0,
                }),
            );
        }
        let direct = encode_fixed_v1(999, &set).unwrap();
        let mut streamed = Vec::new();
        encode_fixed_v1_to_writer(999, &set, &mut streamed).unwrap();
        assert_eq!(direct, streamed);
    }

    #[test]
    fn autodetect_none_for_non_magic() {
        let junk = [0u8; 40];
        assert!(decode_fixed_v1_if_magic(&junk).unwrap().is_none());
    }
}
