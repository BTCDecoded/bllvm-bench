#!/bin/bash
# Helper function to add statistical data to a benchmark JSON entry
# Usage: add_stats_to_benchmark <benchmark_json> <estimates.json_path>
# Returns: Updated benchmark JSON with statistics

set -e

BENCHMARK_JSON="$1"
ESTIMATES_FILE="$2"

if [ ! -f "$ESTIMATES_FILE" ]; then
    echo "$BENCHMARK_JSON"
    exit 0
fi

# Extract stats and merge into benchmark JSON
STATS=$(source "$(dirname "$0")/extract-criterion-stats.sh" "$ESTIMATES_FILE")

# Validate STATS is valid JSON before using --slurpfile
if echo "$STATS" | jq . >/dev/null 2>&1; then
    # Use temp file with --slurpfile (more reliable than --argjson)
    TEMP_STATS=$(mktemp)
    echo "$STATS" > "$TEMP_STATS"
    echo "$BENCHMARK_JSON" | jq --slurpfile stats "$TEMP_STATS" '.statistics = $stats[0]' 2>/dev/null || echo "$BENCHMARK_JSON"
    rm -f "$TEMP_STATS"
else
    echo "$BENCHMARK_JSON"
fi

