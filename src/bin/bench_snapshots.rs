use blvm_protocol::block::connect_block_ibd;
use blvm_protocol::segwit::Witness;
use blvm_protocol::types::{Block, Network, UTXO, UtxoSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

fn load_dump(
    dir: &Path,
) -> Result<(Block, Vec<Vec<Witness>>, UtxoSet), Box<dyn std::error::Error + Send + Sync>> {
    let block: Block = bincode::deserialize_from(std::io::BufReader::new(std::fs::File::open(
        dir.join("block.bin"),
    )?))?;
    let witnesses: Vec<Vec<Witness>> = bincode::deserialize_from(std::io::BufReader::new(
        std::fs::File::open(dir.join("witnesses.bin"))?,
    ))?;
    let raw: std::collections::HashMap<_, UTXO> = bincode::deserialize_from(
        std::io::BufReader::new(std::fs::File::open(dir.join("utxo_set.bin"))?),
    )?;
    let utxo_set: UtxoSet = raw.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
    Ok((block, witnesses, utxo_set))
}

fn validate_snapshot(dir: &Path, height: u64, iterations: u32) -> Vec<f64> {
    let (block, witnesses, utxo_set) = load_dump(dir).expect("load snapshot");
    let block_arc = Arc::new(block.clone());
    let n_txs = block.transactions.len();
    let n_inputs: usize = block.transactions.iter().map(|tx| tx.inputs.len()).sum();

    eprintln!(
        "  height={} txs={} inputs={} utxo_set={} iterations={}",
        height,
        n_txs,
        n_inputs,
        utxo_set.len(),
        iterations,
    );

    let mut times_ms = Vec::with_capacity(iterations as usize);

    for i in 0..iterations {
        let utxo_clone = utxo_set.clone();
        let block_arc_clone = Arc::clone(&block_arc);

        let t = Instant::now();
        let ctx = blvm_protocol::block::block_validation_context_for_connect_ibd(
            None::<&[blvm_protocol::types::BlockHeader]>,
            0u64,
            Network::Mainnet,
        );
        let (result, _new_utxo, _tx_ids, _delta) = connect_block_ibd(
            &block,
            &witnesses,
            utxo_clone,
            height,
            &ctx,
            None,
            None,
            Some(block_arc_clone),
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("connect_block_ibd failed at height {}: {}", height, e));

        let elapsed = t.elapsed().as_secs_f64() * 1000.0;
        times_ms.push(elapsed);

        match result {
            blvm_protocol::ValidationResult::Valid => {}
            blvm_protocol::ValidationResult::Invalid(reason) => {
                panic!("Block {} invalid on iter {}: {}", height, i, reason);
            }
        }
    }

    times_ms
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let snapshot_dir = args.get(1).map(|s| PathBuf::from(s)).unwrap_or_else(|| {
        std::env::var("BLVM_IBD_SNAPSHOT_DIR")
            .map(PathBuf::from)
            .expect("Usage: bench_snapshots <snapshot_dir> [iterations] [height_filter]\n  or set BLVM_IBD_SNAPSHOT_DIR")
    });
    let iterations: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let height_filter: Option<u64> = args.get(3).and_then(|s| s.parse().ok());

    if !snapshot_dir.exists() {
        eprintln!("Error: {} does not exist", snapshot_dir.display());
        std::process::exit(1);
    }

    let mut heights: Vec<u64> = std::fs::read_dir(&snapshot_dir)
        .expect("read snapshot dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let s = name.to_str()?;
            s.strip_prefix("height_")?.parse().ok()
        })
        .collect();
    heights.sort_unstable();

    if let Some(hf) = height_filter {
        heights.retain(|h| *h == hf);
    }
    // Focus on 100k+ for optimization work
    let focus_heights: Vec<u64> = heights.iter().copied().filter(|h| *h >= 100_000).collect();

    eprintln!("=== BLVM Snapshot Benchmark ===");
    eprintln!("Dir: {}", snapshot_dir.display());
    eprintln!("Heights: {:?}", heights);
    eprintln!("Iterations: {}", iterations);
    eprintln!();

    println!("height,txs,inputs,min_ms,median_ms,mean_ms,p95_ms,max_ms,implied_bps");

    for &h in &heights {
        let dir = snapshot_dir.join(format!("height_{}", h));
        if !dir.join("block.bin").exists() {
            eprintln!("Skip {}: block.bin missing", h);
            continue;
        }

        // Warmup
        let _ = validate_snapshot(&dir, h, 1);

        let mut times = validate_snapshot(&dir, h, iterations);
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let min = times[0];
        let max = *times.last().unwrap();
        let median = times[times.len() / 2];
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let p95_idx = ((times.len() as f64) * 0.95) as usize;
        let p95 = times[p95_idx.min(times.len() - 1)];
        let implied_bps = 1000.0 / median;

        println!(
            "{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.1}",
            h,
            // get txs/inputs from the snapshot
            {
                let (block, _, _) = load_dump(&dir).unwrap();
                block.transactions.len()
            },
            {
                let (block, _, _) = load_dump(&dir).unwrap();
                block
                    .transactions
                    .iter()
                    .map(|tx| tx.inputs.len())
                    .sum::<usize>()
            },
            min,
            median,
            mean,
            p95,
            max,
            implied_bps,
        );
    }

    if !focus_heights.is_empty() {
        eprintln!();
        eprintln!("=== 100k+ Summary (optimization target) ===");
        let mut total_median = 0.0;
        let mut count = 0;
        for &h in &focus_heights {
            let dir = snapshot_dir.join(format!("height_{}", h));
            if !dir.join("block.bin").exists() {
                continue;
            }
            let mut times = validate_snapshot(&dir, h, iterations);
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = times[times.len() / 2];
            total_median += median;
            count += 1;
            eprintln!(
                "  height={}: median={:.2}ms ({:.0} bps)",
                h,
                median,
                1000.0 / median
            );
        }
        if count > 0 {
            let avg_median = total_median / count as f64;
            eprintln!(
                "  avg median={:.2}ms ({:.0} bps)",
                avg_median,
                1000.0 / avg_median
            );
            eprintln!(
                "  4x target={:.2}ms ({:.0} bps)",
                avg_median / 4.0,
                4000.0 / avg_median
            );
            eprintln!(
                "  5x target={:.2}ms ({:.0} bps)",
                avg_median / 5.0,
                5000.0 / avg_median
            );
        }
    }
}
