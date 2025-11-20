# Codebase Metrics Proposal

This document outlines proposed codebase metrics for comparing Bitcoin Core and Bitcoin Commons beyond performance benchmarks.

## Metric Categories

### Tier 1: Foundation Metrics (High Priority)
These provide the baseline understanding of codebase size and structure.

#### 1.1 Code Size Metrics
- **Total Lines of Code (LOC)**
  - Raw LOC (all lines)
  - Source Lines of Code (SLOC) - excluding comments/blanks
  - By language (C++ vs Rust)
  - By module/crate
  
- **File Counts**
  - Total source files
  - Files by type (.cpp/.h vs .rs)
  - Files by module/crate
  
- **Code Distribution**
  - LOC per module/crate
  - Largest/smallest modules
  - Module dependency graph

#### 1.2 Code Structure Metrics
- **Function/Method Counts**
  - Total functions
  - Public vs private
  - Average function length
  - Functions per module
  
- **Type Definitions**
  - Struct/class counts
  - Enum counts
  - Trait/interface counts
  - Type complexity (fields per struct)

### Tier 2: Feature & Configuration Metrics (High Priority)
Understanding what features are available and how they're organized.

#### 2.1 Feature Flags
- **Feature Count**
  - Total feature flags
  - Optional features
  - Default features
  - Feature dependencies
  
- **Feature Coverage**
  - Code gated by features
  - Feature combinations
  - Feature usage patterns

#### 2.2 Conditional Compilation
- **Conditional Blocks**
  - `#ifdef` / `#[cfg]` blocks
  - Platform-specific code
  - Build configuration options

### Tier 3: Quality & Safety Metrics (Medium Priority)
Assessing code quality, safety, and maintainability.

#### 3.1 Code Quality
- **Cyclomatic Complexity**
  - Average complexity per function
  - High complexity functions (>10)
  - Complexity distribution
  
- **Code Duplication**
  - Duplicate code percentage
  - Clone detection
  - Refactoring opportunities

#### 3.2 Safety Metrics
- **Memory Safety**
  - Unsafe code blocks (Rust)
  - Manual memory management (C++)
  - Buffer overflow risks
  
- **Type Safety**
  - Type coverage
  - Null safety
  - Type conversion safety

### Tier 4: Testing & Verification Metrics (Medium Priority)
Understanding test coverage and verification approaches.

#### 4.1 Test Coverage
- **Test Metrics**
  - Total test files
  - Test functions/cases
  - Test LOC vs production LOC ratio
  - Test execution time
  
- **Coverage Analysis**
  - Line coverage percentage
  - Branch coverage
  - Function coverage
  - Module coverage

#### 4.2 Verification Metrics
- **Formal Verification** (Commons-specific)
  - Kani proof count
  - Verified functions
  - Verification coverage
  
- **Property-Based Testing**
  - Proptest cases
  - Fuzz targets
  - Test generators

### Tier 5: Documentation Metrics (Low Priority)
Assessing documentation quality and completeness.

#### 5.1 Code Documentation
- **Comment Density**
  - Comments per LOC
  - Documentation comments
  - Inline vs block comments
  
- **API Documentation**
  - Public API documentation coverage
  - Doc comment completeness
  - Example code in docs

#### 5.2 External Documentation
- **Documentation Files**
  - README files
  - Markdown documentation
  - Specification documents
  - Architecture diagrams

### Tier 6: Build & Dependency Metrics (Low Priority)
Understanding build complexity and dependencies.

#### 6.1 Build Metrics
- **Build Time**
  - Full build time
  - Incremental build time
  - Test build time
  
- **Binary Size**
  - Release binary size
  - Debug binary size
  - Stripped binary size

#### 6.2 Dependency Metrics
- **Dependencies**
  - Total dependencies
  - Direct vs transitive
  - Dependency tree depth
  - Security vulnerabilities

## Proposed Implementation

### Tools & Approaches

#### For Code Metrics
1. **`tokei`** (Rust) - Fast, accurate LOC counter
   - Supports C++ and Rust
   - Provides detailed breakdowns
   - JSON output available

2. **`cloc`** (Cross-language) - Comprehensive LOC analysis
   - Handles comments/blanks correctly
   - Supports many languages
   - Detailed reports

3. **`scc`** (Fast) - Very fast code counter
   - Multi-language support
   - Complexity estimates
   - JSON output

#### For Complexity Analysis
1. **`lizard`** (C++/Python) - Cyclomatic complexity
   - Supports C++
   - Can be extended for Rust
   
2. **`rust-code-analysis`** (Rust) - Rust-specific metrics
   - Complexity analysis
   - Maintainability index
   - Code quality metrics

#### For Feature Flags
1. **Custom parsing** - Parse Cargo.toml and CMakeLists.txt
   - Extract feature definitions
   - Track feature usage
   - Analyze feature dependencies

#### For Test Coverage
1. **`cargo-tarpaulin`** (Rust) - Test coverage for Commons
2. **`gcov`/`lcov`** (C++) - Test coverage for Core
3. **`kcov`** - Unified coverage tool

#### For Documentation
1. **`rustdoc`** (Rust) - Extract doc comments
2. **`doxygen`** (C++) - Extract C++ documentation
3. **Custom scripts** - Count markdown files, analyze structure

### Output Format

All metrics should be collected into a JSON structure similar to benchmark results:

```json
{
  "timestamp": "2025-11-20T15:00:00Z",
  "codebase_metrics": {
    "bitcoin_core": {
      "code_size": {
        "total_loc": 540000,
        "sloc": 420000,
        "files": 1437,
        "by_module": { ... }
      },
      "features": {
        "total_features": 15,
        "optional_features": 8,
        "default_features": 7
      },
      "tests": {
        "test_files": 280,
        "test_loc": 45000,
        "coverage_percentage": 75.5
      },
      "documentation": {
        "comment_density": 0.15,
        "doc_files": 45
      }
    },
    "bitcoin_commons": {
      "code_size": {
        "total_loc": 829000,
        "sloc": 650000,
        "files": 644,
        "by_crate": { ... }
      },
      "features": {
        "total_features": 12,
        "optional_features": 5,
        "default_features": 7,
        "cargo_features": true
      },
      "tests": {
        "test_files": 100,
        "test_loc": 120000,
        "coverage_percentage": 85.2,
        "kani_proofs": 45
      },
      "documentation": {
        "comment_density": 0.22,
        "doc_files": 38
      }
    },
    "comparison": {
      "loc_ratio": 1.54,
      "file_ratio": 0.45,
      "test_coverage_delta": 9.7,
      "feature_count_delta": -3
    }
  }
}
```

## Prioritization

### Phase 1: Foundation Metrics (High Priority - Immediate)
**Goal**: Understand codebase size and structure

1. **Code Size Metrics** ⭐⭐⭐
   - Total LOC (raw + SLOC excluding comments/blanks)
   - File counts by type (.cpp/.h vs .rs)
   - LOC per module/crate
   - Module/crate breakdown
   - **Tool**: `tokei` (fast, accurate, JSON output)

2. **Feature Flags Analysis** ⭐⭐⭐
   - Total feature count (Cargo features vs CMake options)
   - Optional vs default features
   - Feature dependencies
   - Code gated by `#[cfg]` / `#ifdef`
   - **Tool**: Custom parsing of `Cargo.toml` and `CMakeLists.txt`

3. **Basic Test Metrics** ⭐⭐
   - Test file counts
   - Test LOC
   - Test-to-production LOC ratio
   - **Tool**: File counting + `tokei` on test directories

### Phase 2: Combined Views (High Priority - Short-term)
**Goal**: See codebase in different scopes

4. **Code + Feature Flags + Tests** ⭐⭐⭐
   - Combined view showing:
     - Production code
     - Feature-gated code (all variants)
     - Test code
   - Shows total codebase size including all build variants
   - **Tool**: Combine Phase 1 metrics

5. **Code + Feature Flags + Tests + Comments** ⭐⭐
   - Full codebase view including documentation
   - Comment density
   - Documentation file counts
   - **Tool**: Extend Phase 1 with comment analysis

### Phase 3: Quality Metrics (Medium Priority)
**Goal**: Assess code quality and maintainability

6. **Complexity Analysis** ⭐⭐
   - Cyclomatic complexity
   - Average function complexity
   - High-complexity functions (>10)
   - **Tool**: `lizard` (C++) + `rust-code-analysis` (Rust)

### Excluded (Separate Workflows)
- ❌ **Test Coverage** - Separate workflow (requires test execution)
- ❌ **Code Duplication** - Not prioritized
- ❌ **Formal Verification Metrics** - Separate workflow (Kani-specific)

### Phase 4: Integration (Long-term)
- Historical tracking
- Trend analysis
- Visualization and reporting

## Implementation Plan

### Step 1: Create Metrics Collection Scripts
- `scripts/metrics/code-size.sh` - LOC, file counts
- `scripts/metrics/features.sh` - Feature flag analysis
- `scripts/metrics/tests.sh` - Test metrics
- `scripts/metrics/complexity.sh` - Complexity analysis

### Step 2: Integrate with Existing System
- Add metrics collection to `run-benchmarks.sh`
- Include in consolidated JSON
- Display on GitHub Pages

### Step 3: Add Comparison Logic
- Calculate ratios and deltas
- Identify significant differences
- Generate comparison insights

### Step 4: Visualization
- Add metrics to HTML report
- Create comparison charts
- Historical trend graphs

## Questions to Resolve

1. **Scope**: Should we include all Commons crates or just core ones?
   - Recommendation: Start with `bllvm-consensus` and `bllvm-node`, expand later

2. **Frequency**: How often should metrics be collected?
   - Recommendation: Same as benchmarks (daily or on push)

3. **Storage**: Separate from benchmarks or integrated?
   - Recommendation: Integrated into consolidated JSON, separate section

4. **Tooling**: Install tools in workflow or use existing?
   - Recommendation: Install lightweight tools (`tokei`, `cloc`) in workflow

5. **Comparison Fairness**: How to handle language differences?
   - Recommendation: Document differences, focus on functional comparisons

