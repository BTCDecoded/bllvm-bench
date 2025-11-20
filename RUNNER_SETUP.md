# Runner Setup Guide

This document describes the required setup for the self-hosted runner to successfully run all benchmarks.

## Required System Packages

Install these packages on the runner:

```bash
# Ubuntu/Debian
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  cmake \
  libssl-dev \
  pkg-config \
  ccache \
  jq \
  timeout \
  git \
  curl \
  libboost-dev \
  libevent-dev

# Arch Linux
sudo pacman -S --noconfirm \
  base-devel \
  cmake \
  openssl \
  pkgconf \
  ccache \
  jq \
  coreutils \
  git \
  curl \
  boost \
  libevent

# Fedora/RHEL
sudo dnf install -y \
  gcc gcc-c++ make \
  cmake \
  openssl-devel \
  pkgconfig \
  ccache \
  jq \
  coreutils \
  git \
  curl \
  boost-devel \
  libevent-devel
```

## Required Rust Setup

The runner needs Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup default stable
```

## Environment Variables

The runner should have these environment variables set (or they will be set by the workflow):

- `CORE_PATH`: Path to Bitcoin Core repository (default: `$HOME/bitcoin-core`)
- `COMMONS_CONSENSUS_PATH`: Path to bllvm-consensus (default: `$HOME/bllvm-consensus`)
- `COMMONS_NODE_PATH`: Path to bllvm-node (default: `$HOME/bllvm-node`)
- `PKG_CONFIG_PATH`: For finding OpenSSL (default: `/usr/lib/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig`)
- `OPENSSL_DIR`: OpenSSL installation directory (default: `/usr`)

## ccache Configuration

**IMPORTANT**: The runner may have `CC=ccache` or `CXX=ccache` set in the environment. This causes issues:

1. **CMake builds**: CMake needs the real compiler path, not ccache. The workflow handles this by detecting ccache and using it as a launcher.

2. **Rust/OpenSSL builds**: OpenSSL's build scripts fail when `CC=ccache` because they try to use ccache as the compiler directly. The workflow unsets `CC`/`CXX` for Rust builds.

If you want to use ccache on the runner, configure it properly:

```bash
# Use ccache as a launcher, not as CC/CXX
export CC="gcc"
export CXX="g++"
export CMAKE_C_COMPILER_LAUNCHER="ccache"
export CMAKE_CXX_COMPILER_LAUNCHER="ccache"
```

Or unset CC/CXX entirely and let the build systems find compilers automatically.

## perf_event_paranoid (for Deep Analysis)

The deep analysis benchmarks use `perf` to collect CPU metrics. This requires:

```bash
# Temporarily allow perf (for current session)
sudo sysctl -w kernel.perf_event_paranoid=-1

# Or permanently (add to /etc/sysctl.conf)
echo "kernel.perf_event_paranoid=-1" | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

**Security Note**: Setting `perf_event_paranoid=-1` allows any user to use perf. For production runners, consider using `CAP_PERFMON` capabilities instead.

## Bitcoin Core Build Requirements

Bitcoin Core requires:
- CMake 3.16+ (or autotools as fallback)
- Boost 1.64+
- libevent
- OpenSSL

The workflow will build `bench_bitcoin` automatically, but you can also build it manually:

```bash
cd $CORE_PATH
cmake -B build -DCMAKE_BUILD_TYPE=Release -DBUILD_BENCH=ON
cmake --build build -t bench_bitcoin
```

## Verification

After setup, verify the runner can:

1. **Find dependencies**:
   ```bash
   pkg-config --modversion openssl
   which cmake
   which cargo
   ```

2. **Build Core**:
   ```bash
   cd $CORE_PATH
   cmake -B build -DBUILD_BENCH=ON
   ```

3. **Build Commons**:
   ```bash
   cd $COMMONS_CONSENSUS_PATH
   cargo build --release
   ```

4. **Run a test benchmark**:
   ```bash
   cd $BLLVM_BENCH_ROOT
   ./scripts/core/utxo-caching-bench.sh
   ```

## Common Issues

### "ccache: invalid option -- 'O'"
**Cause**: `CC=ccache` is set, and OpenSSL build scripts try to use ccache as compiler.

**Fix**: The workflow now unsets `CC`/`CXX` for Rust builds. If running manually, unset them:
```bash
unset CC CXX
cargo build
```

### "Failed to find OpenSSL development headers"
**Cause**: OpenSSL headers not installed or `pkg-config` can't find them.

**Fix**: Install `libssl-dev` (Ubuntu) or `openssl-devel` (Fedora), and set:
```bash
export PKG_CONFIG_PATH="/usr/lib/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig"
export OPENSSL_DIR="/usr"
```

### "bench_bitcoin not found"
**Cause**: Core not built or build failed.

**Fix**: Build Core manually (see above) or check the workflow logs for build errors.

### "get_output_dir: command not found"
**Cause**: Commons script calls `get_output_dir` before sourcing `common.sh`.

**Fix**: This is now fixed in all scripts - they source `common.sh` first.

### "syntax error: unexpected end of file"
**Cause**: Script has unclosed blocks or missing EOF.

**Fix**: All scripts have been fixed. If you see this, check the script for missing `fi` or `EOF`.

## Next Steps

Once the runner is set up, the workflow should run successfully. Monitor the first run for any remaining issues and update this document as needed.

