# Runner Configuration

**Run on runner:**

```bash
# 1. Fix ccache interference (CRITICAL - prevents OpenSSL build errors)
unset CC CXX
export CMAKE_C_COMPILER_LAUNCHER=ccache
export CMAKE_CXX_COMPILER_LAUNCHER=ccache

# 2. Set perf permissions (required for deep analysis benchmarks)
sudo sysctl -w kernel.perf_event_paranoid=-1

# 3. Make permanent (add to ~/.bashrc)
cat >> ~/.bashrc << 'EOF'
unset CC CXX
export CMAKE_C_COMPILER_LAUNCHER=ccache
export CMAKE_CXX_COMPILER_LAUNCHER=ccache
EOF
source ~/.bashrc
```

**Why:**
- `ccache` in `CC/CXX` breaks Rust's OpenSSL build scripts
- `perf_event_paranoid=-1` allows hardware performance counters
- Workflow handles package installation and OpenSSL paths
