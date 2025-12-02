//! Deep Analysis Module
//! Provides low-level performance metrics similar to bench_bitcoin's deep Core analysis
//!
//! This module enables collection of:
//! - CPU cycles and instructions
//! - Cache performance (L1/L2/L3)
//! - Branch prediction
//! - Memory bandwidth
//!
//! For Commons' own performance optimization and understanding.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub cycles: Option<u64>,
    pub instructions: Option<u64>,
    pub ipc: Option<f64>, // Instructions per cycle
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub references: Option<u64>,
    pub misses: Option<u64>,
    pub miss_rate_percent: Option<f64>,
    pub l1_loads: Option<u64>,
    pub l1_misses: Option<u64>,
    pub l2_loads: Option<u64>,
    pub l2_misses: Option<u64>,
    pub l3_loads: Option<u64>,
    pub l3_misses: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BranchMetrics {
    pub instructions: Option<u64>,
    pub misses: Option<u64>,
    pub miss_rate_percent: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeepAnalysisMetrics {
    pub cpu: CpuMetrics,
    pub cache: CacheMetrics,
    pub branch: BranchMetrics,
}

/// Check if perf is available on the system
pub fn perf_available() -> bool {
    Command::new("perf").arg("--version").output().is_ok()
}

/// Run a benchmark with perf instrumentation
/// Returns parsed perf metrics
pub fn run_with_perf(benchmark_cmd: &[&str]) -> Result<DeepAnalysisMetrics, String> {
    if !perf_available() {
        return Err("perf not available".to_string());
    }

    // Build perf command
    let perf_events = vec![
        "cycles",
        "instructions",
        "cache-references",
        "cache-misses",
        "L1-dcache-loads",
        "L1-dcache-load-misses",
        "L1-icache-loads",
        "L1-icache-load-misses",
        "LLC-loads",
        "LLC-load-misses",
        "branch-instructions",
        "branch-misses",
    ];

    let mut perf_cmd = Command::new("perf");
    perf_cmd.arg("stat");
    for event in &perf_events {
        perf_cmd.arg("-e").arg(event);
    }
    perf_cmd
        .arg("-x,")
        .arg("-o")
        .arg("/tmp/perf-deep-analysis.csv");

    // Add the actual benchmark command
    for arg in benchmark_cmd {
        perf_cmd.arg(arg);
    }

    // Run perf
    let output = perf_cmd
        .output()
        .map_err(|e| format!("Failed to run perf: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "perf command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Parse perf CSV output
    parse_perf_csv("/tmp/perf-deep-analysis.csv")
}

fn parse_perf_csv(path: &str) -> Result<DeepAnalysisMetrics, String> {
    use std::fs;
    use std::io::{BufRead, BufReader};

    let file = fs::File::open(path).map_err(|e| format!("Failed to open perf output: {}", e))?;

    let reader = BufReader::new(file);
    let mut metrics = DeepAnalysisMetrics {
        cpu: CpuMetrics {
            cycles: None,
            instructions: None,
            ipc: None,
        },
        cache: CacheMetrics {
            references: None,
            misses: None,
            miss_rate_percent: None,
            l1_loads: None,
            l1_misses: None,
            l2_loads: None,
            l2_misses: None,
            l3_loads: None,
            l3_misses: None,
        },
        branch: BranchMetrics {
            instructions: None,
            misses: None,
            miss_rate_percent: None,
        },
    };

    for line in reader.lines() {
        let line = line.map_err(|e| format!("Failed to read line: {}", e))?;
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 1 {
            continue;
        }

        let value_str = parts[0].trim();
        let value: u64 = value_str.parse().unwrap_or(0);

        if line.contains("cycles") && !line.contains("cache") {
            metrics.cpu.cycles = Some(value);
        } else if line.contains("instructions") && !line.contains("branch") {
            metrics.cpu.instructions = Some(value);
        } else if line.contains("cache-references") {
            metrics.cache.references = Some(value);
        } else if line.contains("cache-misses")
            && !line.contains("L1")
            && !line.contains("L2")
            && !line.contains("L3")
        {
            metrics.cache.misses = Some(value);
        } else if line.contains("L1-dcache-loads") {
            metrics.cache.l1_loads = Some(value);
        } else if line.contains("L1-dcache-load-misses") {
            metrics.cache.l1_misses = Some(value);
        } else if line.contains("LLC-loads") {
            metrics.cache.l3_loads = Some(value);
        } else if line.contains("LLC-load-misses") {
            metrics.cache.l3_misses = Some(value);
        } else if line.contains("branch-instructions") {
            metrics.branch.instructions = Some(value);
        } else if line.contains("branch-misses") {
            metrics.branch.misses = Some(value);
        }
    }

    // Calculate derived metrics
    if let (Some(cycles), Some(instructions)) = (metrics.cpu.cycles, metrics.cpu.instructions) {
        if cycles > 0 {
            metrics.cpu.ipc = Some(instructions as f64 / cycles as f64);
        }
    }

    if let (Some(refs), Some(misses)) = (metrics.cache.references, metrics.cache.misses) {
        if refs > 0 {
            metrics.cache.miss_rate_percent = Some((misses as f64 / refs as f64) * 100.0);
        }
    }

    if let (Some(inst), Some(misses)) = (metrics.branch.instructions, metrics.branch.misses) {
        if inst > 0 {
            metrics.branch.miss_rate_percent = Some((misses as f64 / inst as f64) * 100.0);
        }
    }

    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_available() {
        // Just check it doesn't panic
        let _ = perf_available();
    }
}
