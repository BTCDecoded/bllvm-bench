# Benchmark Page Improvements - Comprehensive Plan

## Current Status

- ✅ JSON loading works (38243 bytes, 25 top-level benchmarks)
- ✅ Summary shows: 76 total, 14 core, 24 commons, 12 comparisons
- ⚠️ Only displaying: 12 comparisons, 12 commons-only, 1 core-only
- ❌ Missing benchmarks and incomplete data display

## Issues Identified

### 1. **Data Structure Mismatch**
- Summary says 76 total benchmarks, but only 25 top-level entries
- Each top-level entry can contain multiple sub-benchmarks
- Need to handle nested structures like `bitcoin_core_block_validation`, `bitcoin_commons_mempool_operations`

### 2. **Detection Logic Too Strict**
- Benchmarks with errors but valid data are being skipped
- Nested data structures not being detected
- Time extraction not finding values in nested objects

### 3. **Time Extraction Incomplete**
- `extractTime()` doesn't search deep enough in nested structures
- Missing support for structures like:
  - `bitcoin_core_block_validation.connect_block_mixed_ecdsa_schnorr.time_per_block_ms`
  - `bitcoin_commons_mempool_operations.accept_to_memory_pool_simple.time_ms`
  - `benchmarks[]` arrays with nested timing data

### 4. **Comparison Detection**
- Not detecting all valid comparisons
- Missing comparisons where data exists but structure is different

## Implementation Plan

### Phase 1: Improve Data Detection (Priority: HIGH)

#### 1.1 Enhanced Benchmark Detection
- [ ] Remove strict error checking - accept benchmarks with partial data
- [ ] Recursively search for any timing data in nested structures
- [ ] Support all known nested structures:
  - `bitcoin_core_block_validation.*`
  - `bitcoin_core_segwit_operations.*`
  - `bitcoin_commons_mempool_operations.*`
  - `bitcoin_commons_segwit_operations.*`
  - `node_sync.*`
  - `rpc_performance.*`

#### 1.2 Deep Time Extraction
- [ ] Create `findNestedTime()` function that recursively searches for:
  - `time_ms`, `time_ns`, `time`
  - `median`, `mean`
  - `point_estimate` in `statistics`
  - `time_per_block_ms`, `time_per_block_ns`
  - Any numeric field that looks like timing data
- [ ] Search up to 10 levels deep
- [ ] Handle arrays of benchmarks

### Phase 2: Improve Display (Priority: HIGH)

#### 2.1 Enhanced Table Rendering
- [ ] Show all detected benchmarks, not just those with perfect data
- [ ] Display nested benchmark data (e.g., show all SegWit variants)
- [ ] Add expandable rows for benchmarks with multiple measurements
- [ ] Show partial data clearly (e.g., "Core: 250ms, Commons: Error")

#### 2.2 Comparison Calculation
- [ ] Calculate comparisons on-the-fly if missing from JSON
- [ ] Handle different time units (ms, ns, seconds)
- [ ] Show speedup ratios even for partial comparisons

### Phase 3: Handle Edge Cases (Priority: MEDIUM)

#### 3.1 Error Handling
- [ ] Show benchmarks with errors in a separate "Partial Data" section
- [ ] Display error messages but still show available data
- [ ] Indicate which side (core/commons) has data

#### 3.2 Missing Data
- [ ] Show "—" for missing data instead of hiding benchmarks
- [ ] Add tooltips explaining why data is missing
- [ ] Group benchmarks by completeness (full, partial, failed)

### Phase 4: UI/UX Improvements (Priority: MEDIUM)

#### 4.1 Better Organization
- [ ] Group benchmarks by category (validation, serialization, etc.)
- [ ] Add filters for: All, Comparisons, Core-only, Commons-only, Partial
- [ ] Show summary statistics per category

#### 4.2 Visual Indicators
- [ ] Color-code benchmarks by status (complete, partial, failed)
- [ ] Add icons for comparison vs single-implementation
- [ ] Show confidence indicators for comparisons

### Phase 5: Performance & Validation (Priority: LOW)

#### 5.1 Data Validation
- [ ] Validate JSON structure on load
- [ ] Warn if summary counts don't match displayed benchmarks
- [ ] Log discrepancies for debugging

#### 5.2 Performance
- [ ] Lazy-load large benchmark lists
- [ ] Virtual scrolling for 100+ benchmarks
- [ ] Cache parsed data structure

## Implementation Details

### Enhanced Time Extraction Function

```javascript
function findNestedTime(obj, depth = 0, maxDepth = 10) {
    if (!obj || depth > maxDepth) return null;
    
    // Direct time fields
    if (obj.time_ms) return { value: obj.time_ms, unit: 'ms' };
    if (obj.time_ns) return { value: obj.time_ns / 1000000, unit: 'ms' };
    if (obj.time) return { value: obj.time, unit: 'ms' };
    if (obj.time_per_block_ms) return { value: obj.time_per_block_ms, unit: 'ms' };
    if (obj.time_per_block_ns) return { value: obj.time_per_block_ns / 1000000, unit: 'ms' };
    
    // Statistics
    if (obj.statistics?.median?.point_estimate) {
        return { value: obj.statistics.median.point_estimate, unit: 'ns' };
    }
    if (obj.statistics?.mean?.point_estimate) {
        return { value: obj.statistics.mean.point_estimate, unit: 'ns' };
    }
    
    // Arrays
    if (Array.isArray(obj.benchmarks) && obj.benchmarks.length > 0) {
        const first = findNestedTime(obj.benchmarks[0], depth + 1, maxDepth);
        if (first) return first;
    }
    
    // Recursive search
    for (const key in obj) {
        if (typeof obj[key] === 'object' && obj[key] !== null) {
            const result = findNestedTime(obj[key], depth + 1, maxDepth);
            if (result) return result;
        }
    }
    
    return null;
}
```

### Enhanced Detection Logic

```javascript
function hasValidData(obj, minKeys = 2) {
    if (!obj || typeof obj !== 'object') return false;
    
    // Count non-metadata keys
    const metadataKeys = ['error', 'timestamp', 'log_file', 'note', 'raw_output'];
    const dataKeys = Object.keys(obj).filter(k => !metadataKeys.includes(k));
    
    // Must have more than just metadata
    if (dataKeys.length < minKeys) return false;
    
    // Check for any timing data
    const timeData = findNestedTime(obj);
    if (timeData) return true;
    
    // Check for nested data structures
    for (const key of dataKeys) {
        if (key.includes('bitcoin_') || key.includes('benchmarks') || 
            key.includes('node_') || key.includes('rpc_')) {
            return true;
        }
    }
    
    return false;
}
```

## Testing Checklist

- [ ] All 25 top-level benchmarks are displayed
- [ ] All 12 comparisons are shown correctly
- [ ] All 14 core benchmarks are shown
- [ ] All 24 commons benchmarks are shown
- [ ] Nested data structures are extracted and displayed
- [ ] Time values are correctly formatted
- [ ] Comparisons calculate speedup correctly
- [ ] Partial data benchmarks are shown appropriately
- [ ] Error messages are displayed but don't hide valid data
- [ ] Summary counts match displayed benchmarks

## Success Criteria

1. **All benchmarks visible**: All 25 top-level benchmarks displayed
2. **All comparisons shown**: All 12 comparisons properly displayed with timing data
3. **Complete data extraction**: Time values extracted from all nested structures
4. **Proper categorization**: Benchmarks correctly categorized as comparison/core-only/commons-only
5. **No data loss**: All available timing data is displayed, even if partial

## Next Steps

1. Implement enhanced time extraction function
2. Update detection logic to be more lenient
3. Add support for nested data structures
4. Test with current JSON data
5. Verify all benchmarks are displayed
6. Add UI improvements for better organization

