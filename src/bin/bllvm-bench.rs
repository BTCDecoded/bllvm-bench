//! bllvm-bench CLI tool
//!
//! Command-line interface for running benchmarks

use anyhow::{Context, Result};
use bllvm_bench::shell;
use clap::{Parser, Subcommand};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "bllvm-bench")]
#[command(about = "Bitcoin Commons BLLVM Benchmarking Suite")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run Rust Criterion benchmarks
    Rust {
        /// Benchmark name (optional, runs all if not specified)
        name: Option<String>,
        /// Enable production mode
        #[arg(long)]
        production: bool,
    },
    /// Run shell-based benchmarks
    Shell {
        /// Run all shell benchmarks
        #[arg(long)]
        all: bool,
        /// Run specific benchmark suite
        #[arg(long)]
        suite: Option<String>,
        /// Run specific benchmark script
        script: Option<String>,
    },
    /// Run all benchmarks (Rust + Shell)
    All {
        /// Enable production mode for Rust benchmarks
        #[arg(long)]
        production: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Rust { name, production } => {
            println!("Running Rust Criterion benchmarks...");
            if production {
                println!("Production mode enabled");
            }

            let mut cmd = Command::new("cargo");
            cmd.arg("bench");

            if production {
                cmd.arg("--features").arg("production");
            }

            if let Some(bench_name) = name {
                cmd.arg("--bench").arg(&bench_name);
                println!("Running benchmark: {}", bench_name);
            } else {
                println!("Running all benchmarks");
            }

            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

            let status = cmd.status().context("Failed to run cargo bench")?;

            if !status.success() {
                anyhow::bail!("Benchmark execution failed");
            }
        }
        Commands::Shell { all, suite, script } => {
            if all {
                shell::run_all()?;
            } else if let Some(suite) = suite {
                println!("Running suite: {}", suite);
                // TODO: Implement suite running
            } else if let Some(script) = script {
                shell::run_benchmark(&script)?;
            } else {
                println!("Please specify --all, --suite, or a script name");
            }
        }
        Commands::All { production } => {
            println!("Running all benchmarks (Rust + Shell)...");

            // Run Rust benchmarks first
            println!("\n=== Running Rust Criterion Benchmarks ===");
            let mut rust_cmd = Command::new("cargo");
            rust_cmd.arg("bench");
            if production {
                rust_cmd.arg("--features").arg("production");
                println!("Production mode enabled for Rust benchmarks");
            }
            rust_cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

            let rust_status = rust_cmd.status().context("Failed to run Rust benchmarks")?;

            if !rust_status.success() {
                eprintln!("Warning: Rust benchmarks failed, continuing with shell benchmarks...");
            }

            // Run shell benchmarks
            println!("\n=== Running Shell-Based Benchmarks ===");
            shell::run_all()?;

            println!("\n✅ All benchmarks completed!");
        }
    }

    Ok(())
}
