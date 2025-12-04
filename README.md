# bllvm-bench

Bitcoin Commons benchmarking suite for comparing Bitcoin Core and Bitcoin Commons performance.

## Quick Start

```bash
# Clone and setup
git clone <repo> && cd bllvm-bench
make all

# Or step by step:
make setup-auto  # Auto-discover and clone dependencies
make bench       # Run benchmarks
make json        # Generate consolidated JSON
make csv         # Generate CSV report
```

## What It Does

- Runs fair benchmarks comparing Bitcoin Core and Bitcoin Commons
- Generates JSON reports with statistical analysis
- Produces CSV files for spreadsheet analysis
- Automatically discovers Core/Commons installations

## Output

- **Individual JSON**: `results/{benchmark}-{timestamp}.json`
- **Consolidated JSON**: `results/benchmark-results-consolidated-{timestamp}.json`
- **CSV Report**: `results/benchmark-results-consolidated-{timestamp}.csv`
- **Suite Summary**: `results/suite-*/summary.json`
- **GitHub Pages Site**: View at `benchmarks.thebitcoincommons.org` (loads JSON dynamically)

## Automated Benchmarks

Benchmarks run manually via GitHub Actions on a self-hosted runner:
- **Manual**: Trigger via Actions tab

Results are automatically:
- Generated as consolidated JSON
- Committed to `docs/data/`
- Published to GitHub Pages
- Released (on scheduled runs)

See [.github/workflows/README.md](.github/workflows/README.md) for setup instructions.

## Documentation

- [README_BENCHMARKING.md](README_BENCHMARKING.md) - Full benchmarking guide
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) - Common issues and solutions
- [docs/REGRESSION_DETECTION.md](docs/REGRESSION_DETECTION.md) - Regression detection & historical tracking
- [docs/](docs/) - Additional documentation
- [.github/workflows/README.md](.github/workflows/README.md) - GitHub Actions setup

## Make Targets

- `make help` - Show all available targets
- `make setup` - Interactive setup
- `make setup-auto` - Auto-setup (clones dependencies)
- `make bench` - Run benchmarks
- `make json` - Generate consolidated JSON
- `make csv` - Generate CSV from JSON
- `make report-full` - Generate both JSON and CSV
- `make deep-analysis` - Deep Commons analysis (CPU cycles, cache, etc.)
- `make update-gh-pages` - Update GitHub Pages with latest JSON
- `make check` - Check if all dependencies are available
- `make validate` - Validate benchmark JSON (FILE=path/to/file.json)
- `make history` - Track benchmark history for trend analysis
- `make regressions` - Detect performance regressions vs baseline
- `make all` - Full workflow (setup + bench + report)
