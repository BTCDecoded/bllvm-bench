# Implementation Explanation: Benchmark Speed Categorization

## How It Works

### Automatic Runs (Scheduled/Push)

**When:** Scheduled daily at 2 AM UTC, or on push to main

**Behavior:** 
- Always runs **all 3 speed categories** (fast, medium, slow)
- Generates **3 separate JSON files**:
  - `benchmark-results-fast.json`
  - `benchmark-results-medium.json`
  - `benchmark-results-slow.json`
- Also runs differential tests and generates:
  - `differential-test-results.json`
- **All 4 JSON files** are released together in `benchmarks-latest` release

### Manual Runs (workflow_dispatch)

**When:** You manually trigger the workflow

**Options:**
- **fast** (default) - Runs only fast benchmarks (~15-20 min)
- **medium** - Runs only medium benchmarks (~45-60 min)
- **slow** - Runs only slow benchmarks (~2-4 hours)
- **all** - Runs all 3 speed categories (same as automatic)

**Behavior:**
- If you choose `fast`, `medium`, or `slow`: Only that category runs, generates 1 JSON file
- If you choose `all`: Runs all 3 categories, generates 3 JSON files
- Differential tests always run (if enabled)
- Selected JSON files are released

## Speed Categories

### Fast Benchmarks (< 2 minutes each)
Quick validation and core operations:
- Block validation
- Transaction validation
- Mempool operations
- Hash operations
- Script verification

**Total time:** ~15-20 minutes

### Medium Benchmarks (2-10 minutes each)
Moderate complexity operations:
- Block serialization
- Compact block encoding
- RPC performance
- Memory efficiency
- Concurrent operations

**Total time:** ~45-60 minutes

### Slow Benchmarks (> 10 minutes each)
Complex, deep analysis:
- Deep analysis
- Node sync
- Full blockchain operations

**Total time:** ~2-4 hours

## Release Output

The `benchmarks-latest` release will contain:

1. **benchmark-results-fast.json** - Fast benchmarks only
2. **benchmark-results-medium.json** - Medium benchmarks only
3. **benchmark-results-slow.json** - Slow benchmarks only
4. **differential-test-results.json** - Differential test results

All 4 files are uploaded together, making it easy to:
- Download just the speed category you need
- Compare results across categories
- Track differential test results alongside benchmarks

## Workflow Logic

```yaml
# Automatic (scheduled/push)
speed = "all" → Generate 3 JSONs (fast, medium, slow)

# Manual (workflow_dispatch)
speed = "fast" → Generate 1 JSON (fast only)
speed = "medium" → Generate 1 JSON (medium only)
speed = "slow" → Generate 1 JSON (slow only)
speed = "all" → Generate 3 JSONs (fast, medium, slow)
```

## Benefits

1. **Faster CI feedback** - Run fast benchmarks for quick validation
2. **Selective testing** - Choose what you need for your use case
3. **Complete coverage** - Automatic runs ensure all benchmarks are tested
4. **Organized output** - Separate JSONs make it easy to analyze specific categories
5. **Differential testing** - Always included to catch consensus divergences

