//! Shell benchmark runner
//!
//! This module provides functionality to run shell-based benchmarks
//! from the benchmarks/ directory.

use crate::utils;
use anyhow::{Context, Result};
use std::process::{Command, Stdio};

/// Run a specific shell benchmark
pub fn run_benchmark(script: &str) -> Result<()> {
    let benchmarks_dir = utils::benchmarks_dir();

    // If script doesn't have .sh extension, try adding it
    let script_name = if script.ends_with(".sh") {
        script.to_string()
    } else {
        format!("{}.sh", script)
    };

    let script_path = benchmarks_dir.join(&script_name);

    if !script_path.exists() {
        anyhow::bail!("Benchmark script not found: {}", script_path.display());
    }

    // Make sure script is executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = std::fs::metadata(&script_path).map(|m| m.permissions()) {
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&script_path, perms);
        }
    }

    println!("Executing: {}", script_path.display());

    let status = Command::new("bash")
        .arg(&script_path)
        .current_dir(&benchmarks_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("Failed to run benchmark: {}", script))?;

    if !status.success() {
        anyhow::bail!(
            "Benchmark script failed with exit code: {:?}",
            status.code()
        );
    }

    println!("✅ Benchmark completed: {}", script_name);
    Ok(())
}

/// Run all shell benchmarks
pub fn run_all() -> Result<()> {
    let benchmarks_dir = utils::benchmarks_dir();

    if !benchmarks_dir.exists() {
        anyhow::bail!(
            "Benchmarks directory not found: {}",
            benchmarks_dir.display()
        );
    }

    println!(
        "Running all shell benchmarks from: {}",
        benchmarks_dir.display()
    );

    // Look for main suite runner scripts
    let suite_scripts = [
        "run-all-fair-fast-benchmarks.sh",
        "comprehensive-suite.sh",
        "run-all.sh",
    ];

    let mut found = false;
    for script in &suite_scripts {
        let script_path = benchmarks_dir.join(script);
        if script_path.exists() {
            println!("Running suite: {}", script);
            run_benchmark(script)?;
            found = true;
            break;
        }
    }

    if !found {
        println!("No suite runner found. Available scripts:");
        // List available scripts
        if let Ok(entries) = std::fs::read_dir(&benchmarks_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".sh") && entry.path().is_file() {
                        println!("  - {}", name);
                    }
                }
            }
        }
    }

    Ok(())
}
