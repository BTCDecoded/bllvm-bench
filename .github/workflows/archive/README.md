# Workflow Archive

This directory contains archived workflow files that are no longer in active use but are preserved for reference.

## Archived Workflows

### `benchmarks-sequential.yml`

**Status:** Archived (replaced by parallel workflow)

**Original Location:** `.github/workflows/benchmarks.yml`

**Date Archived:** 2025-01-21

**Reason:** Replaced with parallel execution workflow to reduce total execution time from ~1 hour to ~15-20 minutes.

**What it did:**
- Ran all benchmarks sequentially in a single job
- Executed `run-benchmarks.sh` which ran benchmarks one after another
- Generated consolidated JSON after all benchmarks completed

**Why it was replaced:**
- Sequential execution was slow (~1 hour total)
- No parallelization possible within single job
- All benchmarks had to wait for previous ones to complete

**New approach:**
- Benchmarks run in parallel using GitHub Actions matrix strategy
- Each benchmark runs in its own job
- Consolidation happens after all parallel jobs complete
- Significantly faster execution time

## Restoring Archived Workflow

If you need to restore the sequential workflow:

```bash
cp .github/workflows/archive/benchmarks-sequential.yml .github/workflows/benchmarks.yml
```

Note: The parallel workflow is recommended for faster execution, but the sequential workflow may be useful for debugging or if parallel execution causes issues.

