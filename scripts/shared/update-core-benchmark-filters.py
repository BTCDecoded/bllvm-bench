#!/usr/bin/env python3
"""Update Core benchmark scripts with detected benchmark names"""
import json
import re
import sys
from pathlib import Path

BENCHMARKS_JSON = Path(__file__).parent / "bench_bitcoin_benchmarks.json"
CORE_SCRIPTS_DIR = Path(__file__).parent.parent / "core"

if not BENCHMARKS_JSON.exists():
    print(f"❌ Benchmark mapping not found: {BENCHMARKS_JSON}", file=sys.stderr)
    print("   Run detect-bench-bitcoin-benchmarks.sh first", file=sys.stderr)
    sys.exit(1)

with open(BENCHMARKS_JSON) as f:
    data = json.load(f)

# Get categories
mempool = [b for b in data.get("categories", {}).get("mempool", []) if b]
transaction = [b for b in data.get("categories", {}).get("transaction", []) if b]
block = [b for b in data.get("categories", {}).get("block", []) if b]
script = [b for b in data.get("categories", {}).get("script", []) if b]
hash_benchmarks = [b for b in data.get("categories", {}).get("hash", []) if b]
encoding = [b for b in data.get("categories", {}).get("encoding", []) if b]
utxo = [b for b in data.get("categories", {}).get("utxo", []) if b]

print("🔄 Updating Core benchmark scripts...")
print(f"  Mempool: {len(mempool)} benchmarks")
print(f"  Transaction: {len(transaction)} benchmarks")
print(f"  Block: {len(block)} benchmarks")
print(f"  Script: {len(script)} benchmarks")
print(f"  Hash: {len(hash_benchmarks)} benchmarks")
print(f"  Encoding: {len(encoding)} benchmarks")
print(f"  UTXO: {len(utxo)} benchmarks")
print()

def update_filter_in_file(filepath, new_filter):
    """Update -filter= pattern in a file"""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
        
        # Replace -filter="..." pattern
        pattern = r'-filter="[^"]*"'
        replacement = f'-filter="{new_filter}"'
        new_content = re.sub(pattern, replacement, content)
        
        if new_content != content:
            with open(filepath, 'w') as f:
                f.write(new_content)
            return True
    except Exception as e:
        print(f"⚠️  Error updating {filepath}: {e}", file=sys.stderr)
    return False

# Update mempool-operations-bench.sh
if mempool:
    script = CORE_SCRIPTS_DIR / "mempool-operations-bench.sh"
    if script.exists():
        filter_str = "|".join(mempool)
        if update_filter_in_file(script, filter_str):
            print(f"✅ Updated {script.name}")

# Update transaction benchmarks
for script_name, bench_pattern in [
    ("transaction-serialization-bench.sh", "serialization"),
    ("transaction-id-bench.sh", "id|txid"),
    ("transaction-sighash-bench.sh", "sighash"),
]:
    script = CORE_SCRIPTS_DIR / script_name
    if script.exists() and transaction:
        # Find matching benchmark
        matching = [b for b in transaction if re.search(bench_pattern, b, re.I)]
        if matching:
            if update_filter_in_file(script, matching[0]):
                print(f"✅ Updated {script_name}")

# Update block benchmarks
block_script = CORE_SCRIPTS_DIR / "block-assembly-bench.sh"
if block_script.exists() and block:
    matching = [b for b in block if "assemble" in b.lower()]
    if matching:
        if update_filter_in_file(block_script, matching[0]):
            print(f"✅ Updated {block_script.name}")

block_ser_script = CORE_SCRIPTS_DIR / "block-serialization-bench.sh"
if block_ser_script.exists() and block:
    matching = [b for b in block if re.search("read|write|deserialize", b, re.I)]
    if matching:
        filter_str = "|".join(matching[:3])  # Limit to 3
        if update_filter_in_file(block_ser_script, filter_str):
            print(f"✅ Updated {block_ser_script.name}")

# Update script-verification-bench.sh
if script:
    script_file = CORE_SCRIPTS_DIR / "script-verification-bench.sh"
    if script_file.exists():
        filter_str = "|".join(script)
        if update_filter_in_file(script_file, filter_str):
            print(f"✅ Updated {script_file.name}")

# Update hash benchmarks
ripemd_script = CORE_SCRIPTS_DIR / "ripemd160-bench.sh"
if ripemd_script.exists() and hash_benchmarks:
    matching = [b for b in hash_benchmarks if "ripemd" in b.lower()]
    if matching:
        if update_filter_in_file(ripemd_script, matching[0]):
            print(f"✅ Updated {ripemd_script.name}")

merkle_script = CORE_SCRIPTS_DIR / "merkle-tree-bench.sh"
if merkle_script.exists() and hash_benchmarks:
    matching = [b for b in hash_benchmarks if "merkle" in b.lower()]
    if matching:
        if update_filter_in_file(merkle_script, matching[0]):
            print(f"✅ Updated {merkle_script.name}")

# Update encoding benchmarks
if encoding:
    encoding_script = CORE_SCRIPTS_DIR / "base58-bech32-bench.sh"
    if encoding_script.exists():
        filter_str = "|".join(encoding)
        if update_filter_in_file(encoding_script, filter_str):
            print(f"✅ Updated {encoding_script.name}")

# Update UTXO benchmarks
if utxo:
    utxo_script = CORE_SCRIPTS_DIR / "utxo-caching-bench.sh"
    if utxo_script.exists():
        filter_str = "|".join(utxo)
        if update_filter_in_file(utxo_script, filter_str):
            print(f"✅ Updated {utxo_script.name}")

print()
print("✅ Update complete!")
