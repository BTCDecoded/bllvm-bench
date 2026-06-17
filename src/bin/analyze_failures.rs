use anyhow::Result;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

fn main() -> Result<()> {
    let failures_file = blvm_bench::block_cache_env::sort_merge_data_dir()?.join("failures.log");
    let file = File::open(&failures_file)?;
    let reader = BufReader::new(file);

    let mut block_counts: HashMap<u64, u64> = HashMap::new();
    let mut sample_failures: Vec<(u64, String)> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.starts_with('#') || !line.contains("Script returned false") {
            continue;
        }

        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 3 {
            if let Ok(block) = parts[0].trim().parse::<u64>() {
                *block_counts.entry(block).or_insert(0) += 1;
                if sample_failures.len() < 20 {
                    sample_failures.push((block, parts[2].trim().to_string()));
                }
            }
        }
    }

    println!("=== Failure Analysis ===");
    println!("\nTop 20 blocks by failure count:");
    let mut sorted: Vec<_> = block_counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (block, count) in sorted.iter().take(20) {
        println!("  Block {block}: {count} failures");
    }

    println!("\n=== Sample Failures ===");
    for (block, details) in sample_failures.iter().take(10) {
        println!("  Block {block}: {details}");
    }

    println!("\n=== Block Range Analysis ===");
    let mut ranges: HashMap<&str, u64> = HashMap::new();
    for block in block_counts.keys() {
        if *block < 100000 {
            *ranges.entry("< 100k").or_insert(0) += 1;
        } else if *block < 200000 {
            *ranges.entry("100k-200k").or_insert(0) += 1;
        } else if *block < 300000 {
            *ranges.entry("200k-300k").or_insert(0) += 1;
        } else if *block < 400000 {
            *ranges.entry("300k-400k").or_insert(0) += 1;
        } else if *block < 500000 {
            *ranges.entry("400k-500k").or_insert(0) += 1;
        } else {
            *ranges.entry("> 500k").or_insert(0) += 1;
        }
    }
    for (range, count) in ranges {
        println!("  {range}: {count} blocks with failures");
    }

    Ok(())
}
