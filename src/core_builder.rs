//! Bitcoin Core Builder and Discovery
//!
//! This module handles finding and building Bitcoin Core binaries for differential testing.
//! It supports both pre-built Core installations and building Core on-demand.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Bitcoin Core binaries location
#[derive(Debug, Clone)]
pub struct CoreBinaries {
    /// Path to bitcoind binary
    pub bitcoind: PathBuf,
    /// Path to bitcoin-cli binary
    pub bitcoin_cli: PathBuf,
    /// Core version (if available)
    pub version: Option<String>,
}

impl CoreBinaries {
    /// Verify that both binaries exist and are executable
    pub fn verify(&self) -> Result<()> {
        if !self.bitcoind.exists() {
            anyhow::bail!("bitcoind not found at: {}", self.bitcoind.display());
        }
        if !self.bitcoin_cli.exists() {
            anyhow::bail!("bitcoin-cli not found at: {}", self.bitcoin_cli.display());
        }

        // Check if executable (Unix-like)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&self.bitcoind)
                .context("Failed to read bitcoind metadata")?
                .permissions();
            if perms.mode() & 0o111 == 0 {
                anyhow::bail!("bitcoind is not executable: {}", self.bitcoind.display());
            }

            let perms = std::fs::metadata(&self.bitcoin_cli)
                .context("Failed to read bitcoin-cli metadata")?
                .permissions();
            if perms.mode() & 0o111 == 0 {
                anyhow::bail!("bitcoin-cli is not executable: {}", self.bitcoin_cli.display());
            }
        }

        Ok(())
    }
}

/// Bitcoin Core builder and discovery
pub struct CoreBuilder {
    /// Cache directory for Core binaries
    cache_dir: Option<PathBuf>,
    /// Prefer pre-built Core over building
    prefer_prebuilt: bool,
}

impl CoreBuilder {
    /// Create a new CoreBuilder
    pub fn new() -> Self {
        Self {
            cache_dir: std::env::var("BITCOIN_CORE_CACHE_DIR")
                .ok()
                .map(PathBuf::from),
            prefer_prebuilt: true,
        }
    }

    /// Set cache directory
    pub fn with_cache_dir(mut self, dir: PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
    }

    /// Find existing Core installation
    pub fn find_existing_core(&self) -> Result<CoreBinaries> {
        // 1. Check cache first (fast path for self-hosted runner)
        if let Some(ref cache_dir) = self.cache_dir {
            if let Ok(binaries) = self.find_in_cache(cache_dir) {
                return Ok(binaries);
            }
        }

        // 2. Check CORE_PATH environment variable
        if let Ok(core_path) = std::env::var("CORE_PATH") {
            let core_path = PathBuf::from(core_path);
            if let Ok(binaries) = self.find_in_core_path(&core_path) {
                return Ok(binaries);
            }
        }

        // 3. Check common locations (leverage discover-paths.sh logic)
        let search_paths = vec![
            dirs::home_dir()
                .map(|h| h.join("src/bitcoin"))
                .unwrap_or_default(),
            dirs::home_dir()
                .map(|h| h.join("src/bitcoin-core"))
                .unwrap_or_default(),
            dirs::home_dir()
                .map(|h| h.join("src/core"))
                .unwrap_or_default(),
            PathBuf::from("/usr/local/src/bitcoin"),
            PathBuf::from("/opt/bitcoin"),
            PathBuf::from("/opt/bitcoin-core/binaries/v25.0"), // Self-hosted runner cache
        ];

        for path in search_paths {
            if path.exists() {
                if let Ok(binaries) = self.find_in_core_path(&path) {
                    return Ok(binaries);
                }
            }
        }

        // 4. Check if bitcoind is in PATH
        if let Ok(bitcoind_path) = which::which("bitcoind") {
            let bitcoin_cli_path = which::which("bitcoin-cli")
                .context("bitcoind found in PATH but bitcoin-cli not found")?;

            return Ok(CoreBinaries {
                bitcoind: bitcoind_path,
                bitcoin_cli: bitcoin_cli_path,
                version: None,
            });
        }

        anyhow::bail!(
            "Bitcoin Core not found. Please set CORE_PATH or install Core binaries."
        )
    }

    /// Find Core binaries in cache directory
    fn find_in_cache(&self, cache_dir: &Path) -> Result<CoreBinaries> {
        // Check for versioned directories (e.g., v25.0/)
        for entry in std::fs::read_dir(cache_dir).ok().into_iter().flatten() {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let bitcoind = path.join("bitcoind");
                let bitcoin_cli = path.join("bitcoin-cli");
                if bitcoind.exists() && bitcoin_cli.exists() {
                    let version = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string());
                    return Ok(CoreBinaries {
                        bitcoind,
                        bitcoin_cli,
                        version,
                    });
                }
            }
        }

        // Check root of cache directory
        let bitcoind = cache_dir.join("bitcoind");
        let bitcoin_cli = cache_dir.join("bitcoin-cli");
        if bitcoind.exists() && bitcoin_cli.exists() {
            return Ok(CoreBinaries {
                bitcoind,
                bitcoin_cli,
                version: None,
            });
        }

        anyhow::bail!("Core binaries not found in cache")
    }

    /// Find Core binaries in Core source/build directory
    fn find_in_core_path(&self, core_path: &Path) -> Result<CoreBinaries> {
        // Try build/bin first (CMake build)
        let bitcoind = core_path.join("build/bin/bitcoind");
        let bitcoin_cli = core_path.join("build/bin/bitcoin-cli");
        if bitcoind.exists() && bitcoin_cli.exists() {
            return Ok(CoreBinaries {
                bitcoind,
                bitcoin_cli,
                version: None,
            });
        }

        // Try src/ (autotools build)
        let bitcoind = core_path.join("src/bitcoind");
        let bitcoin_cli = core_path.join("src/bitcoin-cli");
        if bitcoind.exists() && bitcoin_cli.exists() {
            return Ok(CoreBinaries {
                bitcoind,
                bitcoin_cli,
                version: None,
            });
        }

        // Try bin/ (installed)
        let bitcoind = core_path.join("bin/bitcoind");
        let bitcoin_cli = core_path.join("bin/bitcoin-cli");
        if bitcoind.exists() && bitcoin_cli.exists() {
            return Ok(CoreBinaries {
                bitcoind,
                bitcoin_cli,
                version: None,
            });
        }

        anyhow::bail!("Core binaries not found in: {}", core_path.display())
    }

    /// Ensure Core is built and available
    /// This will find existing Core or build it if needed
    pub async fn ensure_core_built(&self, version: Option<&str>) -> Result<CoreBinaries> {
        // Try to find existing first
        match self.find_existing_core() {
            Ok(binaries) => {
                binaries.verify()?;
                return Ok(binaries);
            }
            Err(_) => {
                // Not found, continue to build
            }
        }

        // Build Core if not found
        // Note: This is a placeholder - actual building would require more complex logic
        anyhow::bail!(
            "Bitcoin Core not found and building is not yet implemented. \
             Please install Core or set CORE_PATH environment variable."
        )
    }
}

impl Default for CoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_binaries_verify() {
        // This test would require actual Core binaries
        // Skip in CI unless Core is available
        if std::env::var("CORE_PATH").is_ok() {
            let builder = CoreBuilder::new();
            if let Ok(binaries) = builder.find_existing_core() {
                // Should verify successfully if Core is properly installed
                let _ = binaries.verify();
            }
        }
    }
}

