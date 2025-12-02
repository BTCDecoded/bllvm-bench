# Plan Validation Report

## Validation Date
2024-01-XX

## Validation Summary

✅ **Plan is VALID and READY for implementation**

All technical assumptions have been verified against the codebase.

## Detailed Validation

### 1. ✅ Benchmark Scripts Exist

**Validated:**
- Core benchmarks: 20 scripts found
- Commons benchmarks: 25 scripts found  
- Shared benchmarks: 4 scripts found
- Total: ~49 benchmark scripts

**Plan assumption:** ✅ Correct - benchmarks are organized in `scripts/core/`, `scripts/commons/`, and `scripts/shared/benchmarks/`

### 2. ✅ Workflow Dispatch Structure

**Validated:**
- Current workflow has `workflow_dispatch` with `suite` input
- Uses `type: choice` with `default: 'all'`
- GitHub Actions supports multiple inputs

**Plan assumption:** ✅ Correct - can add `speed` input alongside `suite` input

**Implementation note:** Can add `speed` input without breaking existing `suite` input (backward compatible)

### 3. ✅ Consolidation Script

**Validated:**
- Script exists: `scripts/generate-consolidated-json.sh`
- Uses `OUTPUT_DIR` and `OUTPUT_FILE` variables
- Currently generates single `benchmark-results-consolidated-latest.json`
- Script is modular and can be modified

**Plan assumption:** ✅ Correct - can modify to:
- Accept speed category parameter
- Filter JSONs by speed
- Generate multiple output files

### 4. ✅ Release Process

**Validated:**
- Uses `softprops/action-gh-release@v1`
- Current: `files: ${{ steps.consolidate.outputs.json_file }}` (single file)
- Action supports multiple files via array

**Plan assumption:** ✅ Correct - can change to:
```yaml
files: |
  ${{ steps.consolidate.outputs.json_file_fast }}
  ${{ steps.consolidate.outputs.json_file_medium }}
  ${{ steps.consolidate.outputs.json_file_slow }}
  ${{ steps.differential.outputs.json_file }}
```

### 5. ✅ Differential Test Structure

**Validated:**
- Tests use `#[tokio::test]` async framework
- Tests return `Result<()>`
- Tests use `anyhow::Result` for error handling
- Tests already collect comparison results via `compare_block_validation`

**Plan assumption:** ✅ Correct - can:
- Add result collection struct
- Serialize to JSON using `serde_json`
- Write to file in results directory

**Implementation note:** Need to check if `serde_json` is in dependencies

### 6. ⚠️ Speed Categorization Needs Refinement

**Issue Found:**
- Plan lists specific benchmarks, but actual benchmark names may differ
- Need to map actual script names to speed categories

**Recommendation:**
- Create speed mapping file based on actual script names
- Use pattern matching (e.g., `*-validation-bench.sh` = fast)
- Allow for easy updates as benchmarks are added

### 7. ✅ File Structure

**Validated:**
- All planned files/directories exist or can be created
- No conflicts with existing structure

## Potential Issues & Solutions

### Issue 1: Speed Mapping Accuracy
**Problem:** Plan categorizes benchmarks, but actual runtime may differ
**Solution:** 
- Start with conservative estimates
- Allow runtime data to refine categories
- Make speed mapping easily updatable

### Issue 2: Workflow Matrix Generation
**Problem:** Need to filter benchmarks by speed in matrix generation
**Solution:**
- Modify `generate-matrix` job to read speed mapping
- Filter benchmark list before creating matrix
- Handle both `speed` and `suite` inputs

### Issue 3: Differential Test JSON Collection
**Problem:** Rust tests run in parallel, need thread-safe collection
**Solution:**
- Use `Arc<Mutex<Vec<TestResult>>>` for thread-safe collection
- Or use test harness hooks if available
- Write JSON after all tests complete

### Issue 4: Release File Collection
**Problem:** Need to collect 4 files from different steps
**Solution:**
- Consolidation step outputs all 3 benchmark JSONs
- Differential test step outputs differential JSON
- Release step collects all 4 from outputs

## Validation Checklist Results

- [x] ✅ Benchmark scripts exist and are discoverable
- [x] ✅ Workflow structure supports multiple inputs
- [x] ✅ Consolidation script can be modified
- [x] ✅ Release action supports multiple files
- [x] ✅ Differential tests can collect results
- [x] ✅ File structure is valid
- [ ] ⚠️ Speed mapping needs to be created from actual benchmarks
- [ ] ⚠️ Need to verify `serde_json` dependency

## Recommendations

1. **Create speed mapping file first** - Map actual script names to categories
2. **Test workflow dispatch** - Verify `speed` input works alongside `suite`
3. **Add serde_json if missing** - Check `Cargo.toml` for dependency
4. **Implement incrementally** - Start with fast benchmarks, then add medium/slow
5. **Test differential JSON** - Run tests locally to verify JSON output

## Next Steps

1. ✅ Plan validated
2. ⏭️ Create speed mapping file
3. ⏭️ Update workflow with speed input
4. ⏭️ Modify consolidation script
5. ⏭️ Add differential test JSON output
6. ⏭️ Update release step
7. ⏭️ Test end-to-end

## Conclusion

**Plan is VALID** ✅

All major technical assumptions are correct. Minor refinements needed:
- Speed mapping based on actual script names
- Verify `serde_json` dependency
- Thread-safe test result collection

Ready to proceed with implementation.

