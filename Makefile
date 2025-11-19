# bllvm-bench Makefile
# Standard make targets for easy usage

.PHONY: help setup bench report clean all

# Default target
help:
	@echo "╔══════════════════════════════════════════════════════════════╗"
	@echo "║  bllvm-bench: Bitcoin Core vs Commons Benchmark Suite          ║"
	@echo "╚══════════════════════════════════════════════════════════╝"
	@echo ""
	@echo "Available targets:"
	@echo "  make setup       - Interactive setup (clone dependencies if needed)"
	@echo "  make setup-auto  - Auto-setup (clones dependencies automatically)"
	@echo "  make bench       - Run all fair benchmarks"
	@echo "  make bench-fast  - Run fast benchmarks only"
	@echo "  make report      - Generate consolidated JSON"
	@echo "  make json        - Generate consolidated JSON"
	@echo "  make csv         - Generate CSV report"
	@echo "  make deep-analysis - Deep Commons analysis (CPU cycles, cache, etc.)"
	@echo "  make update-gh-pages - Update GitHub Pages with latest JSON"
	@echo "  make check       - Check if all dependencies are available"
	@echo "  make validate    - Validate benchmark JSON (FILE=path/to/file.json)"
	@echo "  make history     - Track benchmark history for trend analysis"
	@echo "  make regressions - Detect performance regressions vs baseline"
	@echo "  make all         - Setup + Bench + Report (full workflow)"
	@echo "  make clean       - Clean results directory"
	@echo "  make help        - Show this help message"
	@echo ""

# Interactive setup
setup:
	@./scripts/setup-dependencies.sh

# Auto-setup (non-interactive, clones if not found)
setup-auto:
	@echo "Auto-setting up dependencies..."
	@BLLVM_BENCH_AUTO_SETUP=1 ./scripts/setup-dependencies.sh

# Run benchmarks
bench:
	@./run-benchmarks.sh fair

# Run fast benchmarks
bench-fast:
	@./run-benchmarks.sh fair-fast

# Generate report (JSON only - HTML is served via GitHub Pages)
report:
	./scripts/generate-consolidated-json.sh 2>&1
	@echo ""
	@echo "✅ Consolidated JSON generated"
	@echo "   Upload to GitHub Pages or serve from results/ directory"

# Generate consolidated JSON only
json:
	./scripts/generate-consolidated-json.sh 2>&1

# Generate CSV from consolidated JSON
csv:
	@./scripts/generate-csv.sh

# Update GitHub Pages with latest benchmark data
update-gh-pages:
	@./scripts/update-gh-pages.sh

# Generate both JSON and CSV
report-full: json csv
	@echo ""
	@echo "✅ Complete! Generated:"
	@echo "   - Consolidated JSON: results/benchmark-results-consolidated-*.json"
	@echo "   - CSV report: results/benchmark-results-consolidated-*.csv"

# Deep analysis (Commons-only, low-level metrics)
deep-analysis:
	@./scripts/commons/deep-analysis-bench.sh
	@echo ""
	@echo "✅ Deep analysis complete!"
	@echo "   - JSON: results/commons-deep-analysis-*.json"
	@echo "   - View via GitHub Pages (loads JSON dynamically)"

# Full workflow: setup + bench + report
all: setup-auto bench report
	@echo ""
	@echo "✅ Complete! Results in: results/benchmark-results-consolidated-*.json"
	@echo "   View at: benchmarks.thebitcoincommons.org (GitHub Pages)"

# Clean results
clean:
	@echo "Cleaning results directory..."
	@rm -rf results/*.json results/suite-* 2>/dev/null || true
	@echo "✅ Cleaned"

# Check dependencies
check:
	@./scripts/check-dependencies.sh

# Validate benchmark JSON
validate:
	@if [ -z "$(FILE)" ]; then \
		echo "Usage: make validate FILE=path/to/benchmark.json"; \
		exit 1; \
	fi
	@./scripts/validate-benchmark.sh "$(FILE)"

# Track benchmark history
history:
	@./scripts/track-history.sh
	@echo ""
	@echo "✅ History tracked. View trends in results/history/trends-*.json"

# Detect performance regressions
regressions:
	@./scripts/detect-regressions.sh
	@echo ""
	@echo "✅ Regression analysis complete. Check results/regression-report-*.json"

