# Codebase Metrics Implementation Plan

## Overview

This document outlines the implementation plan for collecting codebase metrics to compare Bitcoin Core and Bitcoin Commons.

## Prioritized Metrics

### Phase 1: Foundation Metrics (High Priority)

#### 1. Code Size Metrics
**What**: Lines of code, file counts, module breakdowns

**Metrics Collected**:
- Total LOC (all lines)
- Source LOC (excluding comments/blanks)
- File counts by type (.cpp/.h/.rs)
- LOC per module/crate
- Module/crate size distribution

**Implementation**:
- Script: `scripts/metrics/code-size.sh`
- Tool: `tokei` (install via `cargo install tokei` or use system package)
- Output: JSON with breakdown by language and module

#### 2. Feature Flags Analysis
**What**: Feature flag counts, structure, and code gating

**Metrics Collected**:
- Total feature count
- Optional vs default features
- Feature dependencies
- Lines of code gated by features (`#[cfg]` / `#ifdef`)
- Feature combinations

**Implementation**:
- Script: `scripts/metrics/features.sh`
- Tool: Custom parsing
  - Parse `Cargo.toml` for Rust features
  - Parse `CMakeLists.txt` for C++ build options
  - Count `#[cfg(feature = "...")]` blocks
  - Count `#ifdef` / `#if defined()` blocks
- Output: JSON with feature breakdown

#### 3. Basic Test Metrics
**What**: Test file counts and LOC

**Metrics Collected**:
- Test file counts
- Test LOC
- Test-to-production LOC ratio
- Test files per module/crate

**Implementation**:
- Script: `scripts/metrics/tests.sh`
- Tool: `tokei` on test directories
- Output: JSON with test metrics

### Phase 2: Combined Views (High Priority)

#### 4. Code + Feature Flags + Tests
**What**: Combined view of all code variants

**Metrics Collected**:
- Production code LOC
- Feature-gated code LOC (all variants)
- Test code LOC
- Total codebase size (all variants combined)

**Implementation**:
- Script: `scripts/metrics/combined-view.sh`
- Tool: Combine metrics from Phase 1
- Output: JSON with combined breakdown

#### 5. Code + Feature Flags + Tests + Comments
**What**: Full codebase including documentation

**Metrics Collected**:
- All from #4
- Comment LOC
- Documentation file counts
- Comment density (comments/LOC ratio)

**Implementation**:
- Script: `scripts/metrics/full-view.sh`
- Tool: Extend Phase 1 with comment analysis
- Output: JSON with full breakdown

### Phase 3: Quality Metrics (Medium Priority)

#### 6. Complexity Analysis
**What**: Cyclomatic complexity metrics

**Metrics Collected**:
- Average complexity per function
- High complexity functions (>10)
- Complexity distribution
- Most complex modules

**Implementation**:
- Script: `scripts/metrics/complexity.sh`
- Tool: 
  - `lizard` for C++ (install via pip: `pip install lizard`)
  - `rust-code-analysis` for Rust (install via cargo)
- Output: JSON with complexity metrics

## Implementation Structure

### Directory Structure
```
scripts/metrics/
├── code-size.sh          # Phase 1.1
├── features.sh           # Phase 1.2
├── tests.sh              # Phase 1.3
├── combined-view.sh      # Phase 2.1
├── full-view.sh          # Phase 2.2
├── complexity.sh         # Phase 3.1
└── shared/
    └── metrics-common.sh # Common functions
```

### Integration Points

1. **Add to `run-benchmarks.sh`**:
   ```bash
   # After benchmarks, collect metrics
   if [ "$COLLECT_METRICS" = "true" ]; then
       ./scripts/metrics/code-size.sh
       ./scripts/metrics/features.sh
       ./scripts/metrics/tests.sh
   fi
   ```

2. **Add to consolidated JSON**:
   - Include metrics in `benchmark-results-consolidated-latest.json`
   - Separate section: `"codebase_metrics": { ... }`

3. **Display on GitHub Pages**:
   - Add metrics section to `docs/index.html`
   - Show comparison tables and charts

## Tool Installation

### Required Tools

1. **tokei** (Code counting)
   ```bash
   # Option 1: Cargo install
   cargo install tokei
   
   # Option 2: System package
   # Ubuntu/Debian: apt-get install tokei
   # Or download from: https://github.com/XAMPPRocky/tokei/releases
   ```

2. **lizard** (C++ complexity)
   ```bash
   pip install lizard
   ```

3. **rust-code-analysis** (Rust complexity)
   ```bash
   cargo install rust-code-analysis
   ```

### Workflow Installation

Add to `.github/workflows/benchmarks.yml`:
```yaml
- name: Install metrics tools
  run: |
    cargo install tokei --locked || echo "tokei already installed"
    pip install lizard || echo "lizard already installed"
    cargo install rust-code-analysis --locked || echo "rust-code-analysis already installed"
```

## Output Format

### Individual Metric JSON
```json
{
  "timestamp": "2025-11-20T15:00:00Z",
  "metric_type": "code_size",
  "bitcoin_core": {
    "total_loc": 540000,
    "sloc": 420000,
    "files": 1437,
    "by_module": {
      "consensus": { "loc": 85000, "files": 120 },
      "node": { "loc": 120000, "files": 200 },
      ...
    }
  },
  "bitcoin_commons": {
    "total_loc": 829000,
    "sloc": 650000,
    "files": 644,
    "by_crate": {
      "bllvm-consensus": { "loc": 450000, "files": 180 },
      "bllvm-node": { "loc": 250000, "files": 150 },
      ...
    }
  },
  "comparison": {
    "loc_ratio": 1.54,
    "file_ratio": 0.45,
    "analysis": "..."
  }
}
```

### Consolidated Metrics JSON
```json
{
  "timestamp": "2025-11-20T15:00:00Z",
  "codebase_metrics": {
    "code_size": { ... },
    "features": { ... },
    "tests": { ... },
    "combined_view": { ... },
    "full_view": { ... },
    "complexity": { ... }
  }
}
```

## Next Steps

1. ✅ Create proposal document (done)
2. ⚠️ Create Phase 1 scripts (code-size, features, tests)
3. ⚠️ Test scripts locally
4. ⚠️ Integrate into `run-benchmarks.sh`
5. ⚠️ Add to consolidated JSON generator
6. ⚠️ Update GitHub Pages display
7. ⚠️ Add to workflow

## Questions Resolved

- ✅ **Scope**: Focus on `bllvm-consensus` and `bllvm-node` for Commons
- ✅ **Frequency**: Same as benchmarks (daily or on push)
- ✅ **Storage**: Integrated into consolidated JSON, separate section
- ✅ **Tooling**: Install lightweight tools in workflow
- ✅ **Test Coverage**: Separate workflow (excluded from main metrics)
- ✅ **Formal Verification**: Separate workflow (excluded from main metrics)

