# Benchmark Speed Categorization & Release Plan

## Overview

This plan implements:
1. **Speed categorization**: Fast, Medium, Slow benchmarks
2. **Workflow dispatch options**: 3 options for fast/medium/slow (fast as default if possible)
3. **Separated JSON releases**: 3 benchmark JSON files (fast, medium, slow) + 1 differential test JSON
4. **Unified release**: All 4 JSON files released together in "benchmarks-latest" release

## Phase 1: Benchmark Speed Categorization

### Fast Benchmarks (< 2 minutes each)
**Quick validation and core operations**
- `block-validation-bench` (Core & Commons)
- `transaction-validation-bench` (Core & Commons)
- `mempool-operations-bench` (Core & Commons)
- `ripemd160-bench`
- `base58-bech32-bench` (Core only)
- `duplicate-inputs-bench` (Core only)

**Estimated time**: ~15-20 minutes total

### Medium Benchmarks (2-10 minutes each)
**Moderate complexity operations**
- `block-serialization-bench`
- `compact-block-encoding-bench`
- `mempool-rbf-bench`
- `segwit-bench`
- `performance-rpc-http` (shared)
- `memory-efficiency-fair` (shared)
- `concurrent-operations-fair` (shared)

**Estimated time**: ~45-60 minutes total

### Slow Benchmarks (> 10 minutes each)
**Complex, deep analysis, full sync**
- `deep-analysis` (shared)
- `sync-performance` (shared)
- `parallel-operations` (shared)
- Any benchmarks requiring full blockchain sync
- Any benchmarks with large dataset processing

**Estimated time**: ~2-4 hours total

## Phase 2: Workflow Changes

### Update `.github/workflows/benchmarks.yml`

1. **Add speed input to workflow_dispatch**:
```yaml
workflow_dispatch:
  inputs:
    speed:
      description: 'Benchmark speed category'
      required: false
      default: 'fast'
      type: choice
      options:
        - fast
        - medium
        - slow
        - all  # Run all speeds (for full suite)
```

2. **Update suite determination logic**:
   - If `speed` is provided, map to benchmark list
   - If `suite` is provided (legacy), use existing logic
   - Default to `fast` if nothing specified

3. **Create speed-to-benchmark mapping**:
   - `fast`: Fast benchmarks list
   - `medium`: Medium benchmarks list
   - `slow`: Slow benchmarks list
   - `all`: All benchmarks (existing behavior)

## Phase 3: Benchmark JSON Separation

### Update Consolidation Step

1. **Separate JSONs by speed**:
   - Collect all benchmark JSONs
   - Categorize each by speed (fast/medium/slow)
   - Generate 3 separate consolidated JSONs:
     - `benchmark-results-fast.json`
     - `benchmark-results-medium.json`
     - `benchmark-results-slow.json`

2. **Include speed metadata**:
   - Add `speed_category` field to each benchmark entry
   - Add `total_benchmarks` per category
   - Add `estimated_duration` per category

## Phase 4: Differential Test JSON Output

### Create Differential Test JSON Generator

1. **Add JSON output to differential tests**:
   - Modify `tests/integration.rs` to collect test results
   - Generate JSON with structure:
     ```json
     {
       "timestamp": "2024-01-01T00:00:00Z",
       "tests": [
         {
           "name": "test_bip30_differential",
           "status": "passed",
           "bllvm_result": "Invalid",
           "core_result": "Invalid",
           "match": true,
           "duration_ms": 1234
         }
       ],
       "summary": {
         "total": 4,
         "passed": 4,
         "failed": 0,
         "matches": 4,
         "divergences": 0
       }
     }
     ```

2. **Add test result collection**:
   - Track each test's result
   - Compare BLLVM vs Core results
   - Record match/divergence status
   - Measure test duration

3. **Output JSON file**:
   - `differential-test-results.json`
   - Saved to results directory
   - Included in artifacts

## Phase 5: Release Process

### Update Release Step

1. **Collect all 4 JSON files**:
   - `benchmark-results-fast.json`
   - `benchmark-results-medium.json`
   - `benchmark-results-slow.json`
   - `differential-test-results.json`

2. **Release all together**:
   - Single release: `benchmarks-latest`
   - All 4 files as release assets
   - Update release body with summary of all 4

3. **Release naming**:
   - Tag: `benchmarks-latest`
   - Name: "Latest Benchmark & Differential Test Results"
   - Body includes:
     - Fast benchmarks summary
     - Medium benchmarks summary
     - Slow benchmarks summary
     - Differential tests summary

## Implementation Steps

### Step 1: Create Speed Mapping File
- File: `scripts/benchmark-speed-map.sh` or `scripts/benchmark-categories.toml`
- Maps benchmark names to speed categories
- Used by workflow and consolidation

### Step 2: Update Workflow
- Add `speed` input to workflow_dispatch
- Update matrix generation to filter by speed
- Update suite determination logic

### Step 3: Update Consolidation Script
- Read speed mapping
- Separate JSONs by speed
- Generate 3 separate consolidated JSONs

### Step 4: Add Differential Test JSON
- Modify `tests/integration.rs` to collect results
- Create JSON output function
- Save to results directory

### Step 5: Update Release Step
- Collect all 4 JSON files
- Upload all to release
- Update release body

## File Structure

```
bllvm-bench/
├── scripts/
│   ├── benchmark-speed-map.sh          # NEW: Speed categorization
│   ├── generate-consolidated-json.sh   # MODIFY: Separate by speed
│   └── differential-test-json.sh       # NEW: Generate differential JSON
├── .github/workflows/
│   └── benchmarks.yml                  # MODIFY: Add speed input
└── tests/
    └── integration.rs                  # MODIFY: Add JSON output
```

## Validation Checklist

- [x] Plan structure is comprehensive
- [x] Speed categories are well-defined
- [x] Workflow dispatch supports default values (fast can be default)
- [x] Consolidation script exists and can be modified
- [x] Differential tests can collect results
- [ ] Speed mapping covers all benchmarks
- [ ] Workflow dispatch has 3 speed options (fast default)
- [ ] Fast benchmarks complete in < 30 minutes
- [ ] Medium benchmarks complete in < 2 hours
- [ ] Slow benchmarks complete in < 6 hours
- [ ] 3 separate benchmark JSONs generated
- [ ] Differential test JSON generated
- [ ] All 4 JSONs released together
- [ ] Release body includes all summaries
- [ ] Backward compatible with existing suite options

## Technical Validation

### ✅ Workflow Dispatch Default Support
GitHub Actions `workflow_dispatch` supports `default` values for `choice` inputs, so `fast` can be the default.

### ✅ Consolidation Script Modification
The existing `generate-consolidated-json.sh` script:
- Already collects all JSON files
- Already categorizes benchmarks
- Can be modified to filter by speed category
- Can generate multiple output files

### ✅ Differential Test JSON Output
Rust test framework supports:
- Custom test output via `#[test]` attributes
- JSON serialization via `serde_json`
- File I/O to write results
- Result collection via test harness

### ✅ Release Process
GitHub Actions `softprops/action-gh-release`:
- Supports multiple files via `files:` array
- Can update existing releases
- Supports release body templates

## Refined Implementation Details

### Speed Mapping Structure
Use a shell script or JSON file that maps benchmark names to categories:
```bash
# scripts/benchmark-speed-map.sh
declare -A SPEED_MAP=(
    ["block-validation-bench"]="fast"
    ["transaction-validation-bench"]="fast"
    ["mempool-operations-bench"]="fast"
    ["block-serialization-bench"]="medium"
    ["performance-rpc-http"]="medium"
    ["deep-analysis-bench"]="slow"
    # ... etc
)
```

### Workflow Matrix Generation
Modify the `generate-matrix` job to:
1. Read speed mapping
2. Filter benchmarks by speed category
3. Generate matrix only for selected speed

### Consolidation by Speed
Modify `generate-consolidated-json.sh` to:
1. Accept speed category as parameter (or detect from suite)
2. Filter JSON files by speed category
3. Generate separate output file per category
4. Run 3 times (once per category) or once with filtering

### Differential Test JSON
Add to `tests/integration.rs`:
1. Global test result collector (static/thread-local)
2. Test result struct with all needed fields
3. JSON serialization at end of test suite
4. Write to `results/differential-test-results.json`

### Release Step Updates
Modify release step to:
1. Collect all 4 JSON files from results directory
2. Upload all 4 files to release
3. Generate release body with summaries from all 4 files

## Testing Plan

1. **Test fast benchmarks**:
   - Run workflow with `speed: fast`
   - Verify only fast benchmarks run
   - Verify `benchmark-results-fast.json` generated

2. **Test medium benchmarks**:
   - Run workflow with `speed: medium`
   - Verify only medium benchmarks run
   - Verify `benchmark-results-medium.json` generated

3. **Test slow benchmarks**:
   - Run workflow with `speed: slow`
   - Verify only slow benchmarks run
   - Verify `benchmark-results-slow.json` generated

4. **Test differential tests**:
   - Run differential tests
   - Verify `differential-test-results.json` generated
   - Verify JSON structure is correct

5. **Test release**:
   - Run full suite
   - Verify all 4 JSONs in release
   - Verify release body includes all summaries

